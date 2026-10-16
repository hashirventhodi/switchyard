import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import type { Spend } from "../schema/Spend";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

// Local times, so midnight is the viewer's midnight whatever the test machine's zone.
const at = (h: number, m = 0, day = 14) => new Date(2026, 9, day, h, m).getTime();

const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/a", branch: "fix/readme", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/b", branch: "feat/login", primary: false } },
];

function session(id: string, worktreePath: string, branch: string): Event {
  const s: Session = {
    id,
    repositoryId: "repo-1",
    worktreePath,
    branch,
    agent: "claudeCode",
    permissionMode: "askFirst",
    model: { type: "default" },
    status: "running",
    startedAtMs: at(9),
  };
  return { type: "sessionStarted", session: s };
}

const tokens = { inputTokens: 40, outputTokens: 310, cacheReadTokens: 24100, cacheWriteTokens: 2100 };
type Cost = Pick<Spend, "reportedUsd" | "estimatedUsd" | "priceTableVersion">;
/** A Session's spend, all of it on one Model. */
const onOneModel = (cost: Cost, model = "claude-opus-5-5"): Spend => ({ ...tokens, ...cost, byModel: [{ model, ...tokens, ...cost }] });
const reported = (usd: number) => onOneModel({ reportedUsd: usd, estimatedUsd: null, priceTableVersion: null });
const estimated = (usd: number) => onOneModel({ reportedUsd: null, estimatedUsd: usd, priceTableVersion: "test-1" }, "gpt-5.6-terra");
const spent = (sessionId: string, atMs: number, spend: Spend): Event => ({ type: "spendRecorded", sessionId, atMs, spend });

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  return fake;
}

function summaryCost() {
  fireEvent.click(screen.getByRole("button", { name: "Session" }));
  const label = within(screen.getByLabelText("Summary")).getByText("Cost", { selector: ".l" });
  return label.parentElement!;
}

test("a CLI-reported cost is shown as is, without a tilde", () => {
  vi.useFakeTimers({ now: at(12) });
  launch(session("session-1", "/wt/a", "fix/readme"), spent("session-1", at(10), reported(0.0412)), spent("session-1", at(11), reported(0.0587)));

  const cost = summaryCost();
  expect(cost.querySelector(".v")!.textContent).toBe("$0.06");
  expect(cost.title).toMatch(/Reported by the CLI: \$0\.0587/);
  expect(cost.title).toMatch(/40 in · 310 out · 24,100 cache reads · 2,100 cache writes/);
});

test("a token-only CLI's cost is an estimate, marked with a tilde and its price table", () => {
  vi.useFakeTimers({ now: at(12) });
  launch(session("session-1", "/wt/a", "fix/readme"), spent("session-1", at(11), estimated(1.6)));

  const cost = summaryCost();
  expect(cost.querySelector(".v")!.textContent).toBe("~$1.60");
  expect(cost.title).toMatch(/Estimated from price table test-1/);
});

test("with tokens but no price, the cost is unknown rather than zero", () => {
  vi.useFakeTimers({ now: at(12) });
  const unpriced = onOneModel({ reportedUsd: null, estimatedUsd: null, priceTableVersion: null }, "mystery-model");
  launch(session("session-1", "/wt/a", "fix/readme"), spent("session-1", at(11), unpriced));

  const cost = summaryCost();
  expect(cost.querySelector(".v")!.textContent).toBe("Unpriced");
  expect(cost.title).toMatch(/No price for mystery-model in the price table/);
  expect(cost.title).toMatch(/40 in · 310 out/);
});

const modelRow = (model: string, inputTokens: number, cost: Cost) => ({
  model,
  inputTokens,
  outputTokens: 0,
  cacheReadTokens: 0,
  cacheWriteTokens: 0,
  ...cost,
});

function spendByModel() {
  const list = screen.getByRole("list", { name: "Spend by Model" });
  return within(list)
    .getAllByRole("listitem")
    .map((row) => [row.textContent, row.title]);
}

test("a Session that used two Models shows their total and what each cost", () => {
  vi.useFakeTimers({ now: at(12) });
  const spend: Spend = {
    ...tokens,
    reportedUsd: 0.0518,
    estimatedUsd: null,
    priceTableVersion: null,
    byModel: [
      modelRow("claude-opus-5-5", 12, { reportedUsd: 0.0431, estimatedUsd: null, priceTableVersion: null }),
      modelRow("claude-haiku-4-5-20251001", 2400, { reportedUsd: 0.0087, estimatedUsd: null, priceTableVersion: null }),
    ],
  };
  launch(session("session-1", "/wt/a", "fix/readme"), spent("session-1", at(11), spend));

  expect(summaryCost().querySelector(".v")!.textContent).toBe("$0.05");
  expect(spendByModel()).toEqual([
    ["claude-opus-5-5 $0.04", expect.stringMatching(/Reported by the CLI: \$0\.0431 · 12 in/)],
    ["claude-haiku-4-5-20251001 $0.01", expect.stringMatching(/Reported by the CLI: \$0\.0087 · 2,400 in/)],
  ]);
});

test("a Model with no price is listed as unpriced with its tokens, and the total says some is unpriced", () => {
  vi.useFakeTimers({ now: at(12) });
  const spend: Spend = {
    ...tokens,
    reportedUsd: null,
    estimatedUsd: 0.3,
    priceTableVersion: "test-4",
    byModel: [
      modelRow("big-model", 10000, { reportedUsd: null, estimatedUsd: 0.3, priceTableVersion: "test-4" }),
      modelRow("mystery-model", 7000, { reportedUsd: null, estimatedUsd: null, priceTableVersion: null }),
    ],
  };
  launch(session("session-1", "/wt/a", "fix/readme"), spent("session-1", at(11), spend));

  const cost = summaryCost();
  expect(cost.querySelector(".v")!.textContent).toBe("~$0.30 + unpriced");
  expect(cost.title).toMatch(/Unpriced: mystery-model/);
  expect(spendByModel()).toEqual([
    ["big-model ~$0.30", expect.stringMatching(/Estimated from price table test-4/)],
    ["mystery-model unpriced", expect.stringMatching(/No price for mystery-model in the price table · 7,000 in/)],
  ]);
  expect(screen.queryByText(/\$0\.00/)).toBeNull();
});

test("a Session on one Model has no breakdown", () => {
  vi.useFakeTimers({ now: at(12) });
  launch(session("session-1", "/wt/a", "fix/readme"), spent("session-1", at(11), reported(0.0412)));

  summaryCost();
  expect(screen.queryByRole("list", { name: "Spend by Model" })).toBeNull();
});

const statusSpend = () => screen.getByTestId("spend-today-status");
const kpiSpend = () => screen.getByTestId("spend-today-kpi");

test("Spend today sums every session, with reported and estimated totals separable in the tooltip", () => {
  vi.useFakeTimers({ now: at(12) });
  launch(
    session("session-1", "/wt/a", "fix/readme"),
    session("session-2", "/wt/b", "feat/login"),
    spent("session-1", at(10), reported(0.5)),
    spent("session-1", at(11), reported(1.25)), // running total: supersedes 0.5
    spent("session-2", at(11), estimated(2)),
  );

  expect(statusSpend().textContent).toBe("~$3.25 today");
  expect(statusSpend().title).toBe("Reported $1.25 · Estimated ~$2.00");
  expect(kpiSpend().textContent).toBe("Spend today~$3.25");
  expect(kpiSpend().title).toBe("Reported $1.25 · Estimated ~$2.00");
});

test("Spend today has no tilde when every cost was reported", () => {
  vi.useFakeTimers({ now: at(12) });
  launch(session("session-1", "/wt/a", "fix/readme"), spent("session-1", at(11), reported(1.25)));

  expect(statusSpend().textContent).toBe("$1.25 today");
  expect(statusSpend().title).toBe("Reported $1.25 · Estimated ~$0.00");
});

test("Spend today resets at local midnight and then counts only what was spent since", () => {
  vi.useFakeTimers({ now: at(23, 58) });
  const { emit } = launch(
    session("session-1", "/wt/a", "fix/readme"),
    spent("session-1", at(23), reported(1)),
    spent("session-1", at(23, 30), reported(1.5)),
  );
  expect(statusSpend().textContent).toBe("$1.50 today");

  act(() => {
    vi.advanceTimersByTime(3 * 60_000);
  });
  expect(statusSpend().textContent).toBe("$0.00 today");

  // The session's running total moves on after midnight: only the increase is today's.
  act(() => emit(spent("session-1", at(0, 5, 15), reported(2))));
  expect(statusSpend().textContent).toBe("$0.50 today");
});

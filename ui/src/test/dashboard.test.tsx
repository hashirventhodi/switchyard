// The Dashboard beyond the KPIs it started with: Changed files, the agent filter on the
// session cards, the worktree occupancy strip and the activity feed (story 19).
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import { fakeCore } from "./fakeCore";

const NOW = 1_760_000_000_000;
beforeEach(() => {
  localStorage.clear();
  vi.useFakeTimers({ now: NOW, toFake: ["Date"] });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: null };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/fix", branch: "fix/readme", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/feat", branch: "feat/login", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/free", branch: "chore/deps", primary: false } },
];
const session = (id: string, branch: string, path: string, agent: Session["agent"], startedAtMs = NOW - 5 * 60_000): Session => ({
  id,
  repositoryId: "repo-1",
  worktreePath: path,
  branch,
  agent,
  permissionMode: "askFirst",
  status: "running",
  startedAtMs,
});
const fix = session("session-1", "fix/readme", "/wt/fix", "claudeCode");
const feat = session("session-2", "feat/login", "/wt/feat", "codex", NOW - 2 * 60_000);
const facts = (path: string, dirty: number): Event => ({
  type: "worktreeFactsChanged",
  worktreePath: path,
  facts: { ahead: 0, behind: 0, dirty, conflicts: [], changedFiles: dirty, commits: 0 },
});

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, { type: "sessionStarted", session: fix }, { type: "sessionStarted", session: feat }, ...events));
  return fake;
}

test("Changed files counts the uncommitted files awaiting review across worktrees, from pushed facts", () => {
  launch(facts("/wt/fix", 3), facts("/wt/feat", 2), facts("/src/acme", 7));
  const kpi = screen.getByLabelText("Changed files: 5");
  expect(kpi.textContent).toContain("awaiting review");
});

test("the session cards filter by agent", () => {
  launch();
  const all = () => screen.getByRole("region", { name: "All sessions" });
  expect(within(all()).getAllByRole("button", { name: /^Open / }).map((b) => b.getAttribute("aria-label"))).toEqual([
    "Open fix/readme",
    "Open feat/login",
  ]);
  const filter = screen.getByRole("radiogroup", { name: "Filter sessions by agent" });
  fireEvent.click(within(filter).getByRole("radio", { name: "OpenAI Codex" }));
  expect(within(all()).getAllByRole("button", { name: /^Open / }).map((b) => b.getAttribute("aria-label"))).toEqual([
    "Open feat/login",
  ]);
  fireEvent.click(within(filter).getByRole("radio", { name: "All" }));
  expect(within(all()).getAllByRole("button", { name: /^Open / })).toHaveLength(2);
});

test("worktree occupancy shows who is in each worktree and offers the free ones", () => {
  launch();
  const strip = screen.getByRole("region", { name: "Worktree occupancy" });
  const rows = within(strip).getAllByRole("listitem");
  expect(rows.map((r) => r.querySelector(".n")?.textContent)).toEqual([
    "acme/main",
    "acme/fix/readme",
    "acme/feat/login",
    "acme/chore/deps",
  ]);
  expect(rows[1].textContent).toContain("CC");
  expect(rows[1].textContent).toContain("Running");
  expect(rows[2].textContent).toContain("CX");

  fireEvent.click(within(rows[1]).getByRole("button"));
  expect(screen.getByRole("navigation", { name: "Views" }).querySelector("[aria-current=page]")?.getAttribute("aria-label")).toBe(
    "Session",
  );
});

test("a free worktree in the strip starts New session there", () => {
  launch();
  const strip = screen.getByRole("region", { name: "Worktree occupancy" });
  fireEvent.click(within(strip).getByRole("button", { name: "Start an agent in acme/chore/deps" }));
  expect(screen.getByRole("dialog", { name: /New session/ })).toBeTruthy();
});

test("the activity feed lists what happened, newest first, with how long ago", () => {
  const { emit } = launch();
  vi.setSystemTime(NOW + 60_000);
  act(() => emit({ type: "sessionStatusChanged", sessionId: "session-1", status: "waiting" }));
  const feed = screen.getByRole("region", { name: "Activity" });
  const lines = within(feed)
    .getAllByRole("listitem")
    .map((l) => l.textContent);
  expect(lines[0]).toContain("Claude Code · fix/readme: needs you");
  expect(lines[0]).toContain("now");
  expect(lines[1]).toContain("OpenAI Codex · feat/login: started");
  expect(lines[1]).toContain("3m ago");
  expect(lines[2]).toContain("Claude Code · fix/readme: started");
});

test("the Needs you section counts with the status's own label", () => {
  const { emit } = launch();
  act(() => emit({ type: "sessionStatusChanged", sessionId: "session-1", status: "waiting" }));
  const section = screen.getByRole("region", { name: "Needs you" });
  expect(section.querySelector(".sec .sub")?.textContent).toBe("1 Needs you · 0 errors");
});

test("the sidebar counts what needs you, overall and per repository, and opens the queue", () => {
  const { emit } = launch();
  const sidebar = () => screen.getByRole("complementary", { name: "Repositories" });
  expect(within(sidebar()).queryByRole("button", { name: /needs? you/ })).toBeNull();

  act(() => emit({ type: "sessionStatusChanged", sessionId: "session-1", status: "waiting" }));
  expect(within(sidebar()).getByRole("button", { name: "1 needs you" })).toBeTruthy();
  expect(within(sidebar()).getByRole("group", { name: "acme" }).querySelector(".repo-row .needs-n")?.textContent).toBe("1 Needs you");

  act(() => emit({ type: "sessionStatusChanged", sessionId: "session-2", status: "waiting" }));
  fireEvent.click(within(sidebar()).getByRole("button", { name: "2 need you" }));
  expect(screen.getByRole("dialog", { name: /Needs you/ })).toBeTruthy();
});

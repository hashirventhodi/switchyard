// Two agents side by side: a Claude Code and an OpenAI Codex session in two worktrees, each
// shown with its own identity hue, status and counts, every request answerable from the
// Dashboard; and Codex chosen in New session.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { AgentKind } from "../schema/AgentKind";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const ready: Event = {
  type: "agentsChecked",
  agents: [
    { agent: "claudeCode", name: "Claude Code", state: { type: "ready", version: "2.1.300" } },
    { agent: "codex", name: "Codex", state: { type: "ready", version: "0.144.1" } },
  ],
};
const repoEvents: Event[] = [
  { type: "attached" },
  ready,
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/docs", branch: "fix/docs", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/tests", branch: "feat/tests", primary: false } },
];
const session = (id: string, agent: AgentKind, branch: string, path: string): Session => ({
  id,
  repositoryId: "repo-1",
  worktreePath: path,
  branch,
  agent,
  permissionMode: "askFirst",
  model: { type: "default" },
  status: "running",
  startedAtMs: Date.now(),
});
const asks = (sessionId: string, requestId: string, command: string): Event[] => [
  {
    type: "transcriptEntry",
    sessionId,
    index: 0,
    entry: { type: "request", requestId, request: { type: "command", command, cwd: null }, reason: null, state: "pending", answer: null },
  },
  { type: "sessionStatusChanged", sessionId, status: "waiting" },
];

function launch() {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() =>
    fake.emit(
      ...repoEvents,
      { type: "sessionStarted", session: session("session-1", "claudeCode", "fix/docs", "/wt/acme/docs") },
      { type: "sessionStarted", session: session("session-2", "codex", "feat/tests", "/wt/acme/tests") },
    ),
  );
  return fake;
}

const cards = () => within(screen.getByRole("region", { name: "All sessions" }));
const card = (branch: string) => cards().getByText(branch).closest(".card") as HTMLElement;
const hue = (el: HTMLElement) => (el.querySelector(".agent-chip i") as HTMLElement).style.background;

test("each session's card names its agent in that agent's hue", () => {
  launch();

  expect(within(card("fix/docs")).getByText("Claude Code")).toBeTruthy();
  expect(within(card("feat/tests")).getByText("OpenAI Codex")).toBeTruthy();
  expect(hue(card("fix/docs"))).toBe("var(--ag-claude)");
  expect(hue(card("feat/tests"))).toBe("var(--ag-codex)");
});

test("statuses and counts follow each session on its own, and both requests are answerable from the Dashboard", () => {
  const { emit, sent } = launch();
  const statusbar = () => within(screen.getByRole("contentinfo"));
  expect(statusbar().getByText(/running/).textContent).toBe("2 running");

  act(() => emit(...asks("session-2", "0", "pnpm test")));
  expect(within(card("feat/tests")).getByTestId("status").dataset.status).toBe("waiting");
  expect(within(card("fix/docs")).getByTestId("status").dataset.status).toBe("running");
  expect(statusbar().getByText(/running/).textContent).toBe("1 running");
  expect(statusbar().getByText(/Needs you/).textContent).toBe("1 Needs you");

  act(() => emit(...asks("session-1", "req-1", "pnpm lint")));
  expect(screen.getByRole("button", { name: /^Needs you: \d+$/ }).querySelector(".v")?.textContent).toBe("2");
  const needsYou = within(screen.getByRole("region", { name: "Needs you" }));
  expect(needsYou.getByText("Wants to run: pnpm test")).toBeTruthy();
  expect(needsYou.getByText("Wants to run: pnpm lint")).toBeTruthy();
  // One toast per request, each naming its agent; Run once answers without leaving the Dashboard.
  const toasts = [...screen.getByRole("status").querySelectorAll<HTMLElement>(".toast")];
  expect(toasts.map((t) => t.textContent?.split(" wants")[0])).toEqual(["OpenAI Codex on feat/tests", "Claude Code on fix/docs"]);
  for (const toast of toasts) fireEvent.click(within(toast).getByRole("button", { name: "Run once" }));

  expect(sent).toContainEqual({ type: "answerRequest", sessionId: "session-2", requestId: "0", verdict: { type: "allowOnce" } });
  expect(sent).toContainEqual({ type: "answerRequest", sessionId: "session-1", requestId: "req-1", verdict: { type: "allowOnce" } });
});

test("the session view shows the agent's avatar in its hue", () => {
  launch();

  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /feat\/tests/ }));

  const head = screen.getByRole("region", { name: "Session feat/tests" }).querySelector(".lane-head .av") as HTMLElement;
  expect(head.textContent).toBe("CX");
  expect(head.style.background).toBe("var(--ag-codex)");
});

test("OpenAI Codex can be chosen in New session", () => {
  const { sent } = launch();
  fireEvent.keyDown(window, { key: "n", metaKey: true });
  const dialog = within(screen.getByRole("dialog", { name: "New session" }));

  fireEvent.change(dialog.getByLabelText("Agent"), { target: { value: "codex" } });
  fireEvent.change(dialog.getByLabelText("Prompt"), { target: { value: "Add a billing test" } });
  fireEvent.click(dialog.getByRole("button", { name: /^Start/ }));

  const start = sent.find((c) => c.type === "startSessionOnNewWorktree");
  expect(start).toMatchObject({ agent: "codex", prompt: "Add a billing test" });
});

test("a Codex CLI that is missing blocks only Codex in New session", () => {
  const { emit } = launch();
  act(() =>
    emit({
      type: "agentsChecked",
      agents: [
        { agent: "claudeCode", name: "Claude Code", state: { type: "ready", version: "2.1.300" } },
        {
          agent: "codex",
          name: "Codex",
          state: { type: "missing", installHint: "brew install --cask codex", link: { label: "How to install Codex", url: "https://developers.openai.com/codex/cli" } },
        },
      ],
    }),
  );
  fireEvent.keyDown(window, { key: "n", metaKey: true });
  const dialog = within(screen.getByRole("dialog", { name: "New session" }));
  expect(dialog.queryByRole("alert")).toBeNull();

  fireEvent.change(dialog.getByLabelText("Agent"), { target: { value: "codex" } });

  expect(dialog.getByRole("alert").textContent).toContain("brew install --cask codex");
});

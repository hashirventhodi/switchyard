// The box after the transcript when a session stops: what went wrong or what is ready, and
// what to do next (story 39).
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Entry } from "../schema/Entry";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import type { SessionStatus } from "../schema/SessionStatus";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: "origin" };
const fix: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  model: { type: "default" },
  status: "running",
  startedAtMs: 0,
};
const entry = (index: number, e: Entry): Event => ({ type: "transcriptEntry", sessionId: "session-1", index, entry: e });

function launch(status: SessionStatus, ...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() =>
    fake.emit(
      { type: "attached" },
      { type: "repositoryAdded", repository: acme },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/fix", branch: "fix/readme", primary: false } },
      { type: "sessionStarted", session: fix },
      entry(0, { type: "user", text: "Fix the README" }),
      ...events,
      { type: "sessionStatusChanged", sessionId: "session-1", status },
    ),
  );
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  return fake;
}

const box = (name: string) => screen.getByRole("group", { name });
const buttons = (name: string) => within(box(name)).getAllByRole("button").map((b) => b.textContent);

test("Error offers Retry last command, Tell agent to fix and continue, and Open terminal", () => {
  const { sent } = launch(
    "failed",
    entry(1, { type: "tool", callId: "t1", tool: "Bash", summary: "pnpm test", state: "failed", output: "1 failing" }),
    entry(2, { type: "system", text: "Claude Code exited with code 1: out of memory" }),
  );
  expect(buttons("Error")).toEqual(["Retry last command", "Tell agent to fix and continue", "Open terminal"]);
  expect(box("Error").textContent).toContain("out of memory");

  fireEvent.click(within(box("Error")).getByRole("button", { name: "Retry last command" }));
  expect(sent).toContainEqual({ type: "sendPrompt", sessionId: "session-1", text: "Run `pnpm test` again." });
  fireEvent.click(within(box("Error")).getByRole("button", { name: "Tell agent to fix and continue" }));
  expect(sent).toContainEqual({ type: "sendPrompt", sessionId: "session-1", text: "Fix the error above and continue." });
  fireEvent.click(within(box("Error")).getByRole("button", { name: "Open terminal" }));
  expect(sent).toContainEqual({ type: "openShell", worktreePath: "/wt/fix", cols: expect.any(Number), rows: expect.any(Number) });
});

test("Finished offers Review changes, Open pull request and Resume", () => {
  const { sent } = launch("done");
  expect(buttons("Finished")).toEqual(["Review changes", "Open pull request", "Resume session"]);
  fireEvent.click(within(box("Finished")).getByRole("button", { name: "Resume session" }));
  expect(sent).toContainEqual({ type: "resumeSession", sessionId: "session-1" });
  fireEvent.click(within(box("Finished")).getByRole("button", { name: "Review changes" }));
  expect(screen.getByRole("tab", { name: /Changes/ }).getAttribute("aria-selected")).toBe("true");
});

test("Paused offers Resume and Review changes", () => {
  const { sent } = launch("idle");
  expect(buttons("Paused")).toEqual(["Resume session", "Review changes"]);
  fireEvent.click(within(box("Paused")).getByRole("button", { name: "Resume session" }));
  expect(sent).toContainEqual({ type: "resumeSession", sessionId: "session-1" });
});

test("slash-command chips fill the composer for the user to send", () => {
  const { sent } = launch("waiting");
  const chips = screen.getByRole("group", { name: "Slash commands" });
  expect(within(chips).getAllByRole("button").map((b) => b.textContent)).toEqual([
    "/review changes",
    "/commit",
    "/test",
    "/explain this diff",
    "/open pr",
  ]);
  fireEvent.click(within(chips).getByRole("button", { name: "/test" }));
  const composer = screen.getByRole("textbox", { name: "Message the agent" }) as HTMLTextAreaElement;
  expect(composer.value).toBe("/test");
  expect(document.activeElement).toBe(composer);
  expect(sent.some((c) => c.type === "sendPrompt")).toBe(false);
});

test("a running session has no box", () => {
  launch("running");
  for (const name of ["Error", "Finished", "Paused"]) expect(screen.queryByRole("group", { name })).toBeNull();
});

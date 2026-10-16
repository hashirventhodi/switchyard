// Worktree facts are pushed by the core (at attach and on every change on disk, ADR-0008):
// the counters and badges that read them move on `worktreeFactsChanged` alone, without any
// view being opened first and without the renderer asking git for anything.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import type { WorktreeFacts } from "../schema/WorktreeFacts";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: null };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/fix", branch: "fix/readme", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/feat", branch: "feat/login", primary: false } },
];
const done: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  model: { type: "default" },
  status: "done",
  startedAtMs: 0,
};
const facts = (path: string, f: Partial<WorktreeFacts>): Event => ({
  type: "worktreeFactsChanged",
  worktreePath: path,
  facts: { ahead: 0, behind: 0, dirty: 0, conflicts: [], changedFiles: 0, commits: 0, ...f },
});

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  return fake;
}

const sidebar = () => screen.getByRole("complementary", { name: "Repositories" });

test("the Dashboard's Changed files moves with pushed facts and asks git for nothing", () => {
  const { sent, emit } = launch(facts("/wt/fix", { dirty: 1, changedFiles: 1 }));
  expect(screen.getByLabelText("Changed files: 1")).toBeTruthy();

  // What Changes would list, not git's dirty count (a staged-only file is dirty, not listed).
  act(() => emit(facts("/wt/fix", { dirty: 4, changedFiles: 3 }), facts("/wt/feat", { dirty: 2, changedFiles: 2 })));

  expect(screen.getByLabelText("Changed files: 5")).toBeTruthy();
  expect(sent).not.toContainEqual({ type: "refreshWorktrees" });
});

test("the sidebar's behind badge follows pushed facts with no view opened", () => {
  const { emit } = launch();
  expect(within(sidebar()).queryByText(/↓/)).toBeNull();

  act(() => emit(facts("/wt/feat", { behind: 2 })));
  expect(within(sidebar()).getByText("↓2")).toBeTruthy();

  act(() => emit(facts("/wt/feat", { behind: 0 })));
  expect(within(sidebar()).queryByText(/↓/)).toBeNull();
});

test("commits ready shows the facts pushed before the session was opened and follows later ones", () => {
  const { emit } = launch({ type: "sessionStarted", session: done }, facts("/wt/fix", { ahead: 2 }));

  fireEvent.click(within(sidebar()).getByRole("button", { name: /fix\/readme/ }));
  act(() => emit({ type: "changesRead", worktreePath: "/wt/fix", files: [] }));
  expect(screen.getByText("Working tree clean. 2 commits ready.")).toBeTruthy();

  act(() => emit(facts("/wt/fix", { ahead: 3 })));
  expect(screen.getByText("Working tree clean. 3 commits ready.")).toBeTruthy();
});

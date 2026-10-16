// Inline conflict resolve in the Changes tab: conflict hunks with base's lines highlighted,
// Keep ours / Keep base / Ask agent to rebase, and Commit held back until no conflict is left.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { ChangedFile } from "../schema/ChangedFile";
import type { ConflictFile } from "../schema/ConflictFile";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const WT = "/wt/acme/fix";
const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const session: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: WT,
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  model: { type: "default" },
  status: "waiting",
  startedAtMs: 0,
};
const start: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: WT, branch: "fix/readme", primary: false } },
  { type: "sessionStarted", session },
];

const lib: ChangedFile = {
  path: "lib.txt",
  oldPath: null,
  status: "modified",
  added: 1,
  removed: 1,
  binary: false,
  decision: "accept",
  stale: false,
  hunks: [],
  staleHunks: [],
};
const unmergedReadme: ChangedFile = { ...lib, path: "README", status: "conflicted", decision: null };
const readmeConflicts: ConflictFile = {
  path: "README",
  kind: "unmerged",
  hunks: [
    { id: "c1", before: ["one"], ours: ["TWO ours"], base: ["TWO base"], after: ["three"] },
    { id: "c2", before: ["seven"], ours: ["EIGHT ours"], base: ["EIGHT base"], after: ["nine"] },
  ],
};

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...start, ...events));
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  return fake;
}

const pane = () => screen.getByRole("complementary", { name: "Review" });
const commitButton = () => within(pane()).getByRole("button", { name: "Commit accepted…" }) as HTMLButtonElement;
const conflictHunks = () => within(pane()).getAllByRole("group", { name: /^Conflict with main/ });
const toasts = () => Array.from(document.querySelectorAll(".toast .tx")).map((t) => t.textContent ?? "");

test("an unmerged file's conflict hunks show the branch's lines and base's lines highlighted", () => {
  launch(
    { type: "changesRead", worktreePath: WT, files: [unmergedReadme, lib] },
    { type: "conflictsRead", worktreePath: WT, base: "main", files: [readmeConflicts] },
  );

  const hunks = conflictHunks();
  expect(hunks).toHaveLength(2);
  const baseLines = Array.from(hunks[0].querySelectorAll(".ln.base")).map((l) => l.textContent);
  const ourLines = Array.from(hunks[0].querySelectorAll(".ln.ours")).map((l) => l.textContent);
  expect(baseLines).toEqual([expect.stringContaining("TWO base")]);
  expect(ourLines).toEqual([expect.stringContaining("TWO ours")]);
  expect(hunks[0].textContent).toContain("one");
  expect(hunks[0].textContent).toContain("three");
  expect(within(hunks[0]).getByRole("button", { name: "Keep readme" })).toBeTruthy();
  expect(within(hunks[0]).getByRole("button", { name: "Keep main" })).toBeTruthy();
  expect(commitButton().disabled).toBe(true);
});

test("Keep ours and Keep base resolve through the core, and Commit re-enables when no conflict is left", () => {
  const { sent, emit } = launch(
    { type: "changesRead", worktreePath: WT, files: [unmergedReadme, lib] },
    { type: "conflictsRead", worktreePath: WT, base: "main", files: [readmeConflicts] },
  );

  fireEvent.click(within(conflictHunks()[0]).getByRole("button", { name: "Keep main" }));
  expect(sent).toContainEqual({ type: "resolveConflict", worktreePath: WT, path: "README", hunkId: "c1", side: "base" });
  act(() =>
    emit(
      { type: "conflictResolved", worktreePath: WT, path: "README", side: "base", remaining: 1 },
      { type: "changesRead", worktreePath: WT, files: [unmergedReadme, lib] },
      { type: "conflictsRead", worktreePath: WT, base: "main", files: [{ ...readmeConflicts, hunks: [readmeConflicts.hunks[1]] }] },
    ),
  );
  expect(toasts()).toContain("Kept main in README · 1 conflict left");
  expect(conflictHunks()).toHaveLength(1);
  expect(commitButton().disabled).toBe(true);

  fireEvent.click(within(conflictHunks()[0]).getByRole("button", { name: "Keep readme" }));
  expect(sent).toContainEqual({ type: "resolveConflict", worktreePath: WT, path: "README", hunkId: "c2", side: "ours" });
  act(() =>
    emit(
      { type: "conflictResolved", worktreePath: WT, path: "README", side: "ours", remaining: 0 },
      { type: "changesRead", worktreePath: WT, files: [lib] },
      { type: "conflictsRead", worktreePath: WT, base: "main", files: [] },
    ),
  );
  expect(toasts()).toContain("Resolved README, keeping fix/readme");
  expect(within(pane()).queryByRole("group", { name: "Conflicts" })).toBeNull();
  expect(commitButton().disabled).toBe(false);
});

test("a conflict only with base shows its hunks but offers only rebasing", () => {
  const { sent } = launch(
    { type: "changesRead", worktreePath: WT, files: [lib] },
    { type: "worktreeFactsChanged", worktreePath: WT, facts: { ahead: 1, behind: 1, dirty: 1, conflicts: ["README"], changedFiles: 1, commits: 1 } },
    { type: "conflictsRead", worktreePath: WT, base: "main", files: [{ ...readmeConflicts, kind: "base" }] },
  );

  const hunks = conflictHunks();
  expect(hunks).toHaveLength(2);
  expect(Array.from(hunks[0].querySelectorAll(".ln.base")).map((l) => l.textContent)).toEqual([expect.stringContaining("TWO base")]);
  expect(within(hunks[0]).queryByRole("button", { name: /^Keep / })).toBeNull();
  fireEvent.click(within(hunks[0]).getByRole("button", { name: "Ask agent to rebase" }));
  expect(sent).toContainEqual({ type: "askAgentToRebase", sessionId: "session-1" });
  expect(commitButton().disabled).toBe(true);
});

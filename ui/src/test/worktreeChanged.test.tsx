// The core pushes `worktreeChanged` beside a worktree's facts (ADR-0008): the view on screen
// for that worktree re-reads its heavy data on the kinds it shows, nothing else asks, and
// the Changes and History badges read the pushed facts alone.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { ChangeKind } from "../schema/ChangeKind";
import type { Command } from "../schema/Command";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import type { WorktreeFacts } from "../schema/WorktreeFacts";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const WT = "/wt/fix";
const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: null };
const session: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: WT,
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  model: { type: "default" },
  status: "running",
  startedAtMs: 0,
};
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: WT, branch: "fix/readme", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/feat", branch: "feat/login", primary: false } },
  { type: "sessionStarted", session },
];
const facts = (path: string, f: Partial<WorktreeFacts>): Event => ({
  type: "worktreeFactsChanged",
  worktreePath: path,
  facts: { ahead: 0, behind: 0, dirty: 0, conflicts: [], changedFiles: 0, commits: 0, ...f },
});
const changed = (path: string, ...kinds: ChangeKind[]): Event => ({ type: "worktreeChanged", worktreePath: path, kinds });

function openSession(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  return fake;
}

const pane = () => screen.getByRole("complementary", { name: "Review" });
const tab = (name: RegExp) => within(pane()).getByRole("tab", { name });
const count = (sent: Command[], type: Command["type"]) => sent.filter((c) => c.type === type).length;

test("with Changes open, a files change re-reads the changes; a change elsewhere does not", () => {
  const { sent, emit } = openSession();
  const before = count(sent, "readChanges");

  act(() => emit(changed(WT, "files")));
  expect(count(sent, "readChanges")).toBe(before + 1);
  expect(sent[sent.length - 1]).toEqual({ type: "readChanges", worktreePath: WT });

  act(() => emit(changed(WT, "git")));
  expect(count(sent, "readChanges")).toBe(before + 2);

  act(() => emit(changed(WT, "config"), changed("/wt/feat", "files", "git")));
  expect(count(sent, "readChanges")).toBe(before + 2);
  expect(count(sent, "readHistory")).toBe(0);
});

test("with History open, a git change re-reads the history; a files change does not", () => {
  const { sent, emit } = openSession();
  fireEvent.click(tab(/History/));
  const history = count(sent, "readHistory");
  const changes = count(sent, "readChanges");

  act(() => emit(changed(WT, "git")));
  expect(count(sent, "readHistory")).toBe(history + 1);

  act(() => emit(changed(WT, "files")));
  expect(count(sent, "readHistory")).toBe(history + 1);
  expect(count(sent, "readChanges")).toBe(changes);
});

test("with neither tab open nothing is read, and the badges still follow the facts", () => {
  const { sent, emit } = openSession(facts(WT, { changedFiles: 2, commits: 1, dirty: 2, ahead: 1 }));
  fireEvent.click(tab(/Context/));
  expect(tab(/Changes/).textContent).toContain("2");
  expect(tab(/History/).textContent).toContain("1");
  const reads = count(sent, "readChanges") + count(sent, "readHistory");

  act(() => emit(facts(WT, { changedFiles: 5, commits: 3, dirty: 5, ahead: 3 }), changed(WT, "files", "git")));

  expect(count(sent, "readChanges") + count(sent, "readHistory")).toBe(reads);
  expect(tab(/Changes/).textContent).toContain("5");
  expect(tab(/History/).textContent).toContain("3");
});

test("the History count shows before the History tab was ever opened", () => {
  openSession(facts(WT, { commits: 4, ahead: 4 }));

  expect(tab(/History/).textContent).toContain("4");
  expect(tab(/Changes/).textContent).toContain("0");
});

test("a status change no longer re-reads the changes; the signal does", () => {
  const { sent, emit } = openSession();
  const before = count(sent, "readChanges");

  act(() => emit({ type: "sessionStatusChanged", sessionId: "session-1", status: "done" }));

  expect(count(sent, "readChanges")).toBe(before);
});

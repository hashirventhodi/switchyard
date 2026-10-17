import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/fix", branch: "fix/readme", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/feat", branch: "feat/login", primary: false } },
];
const session: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  model: { type: "default" },
  status: "waiting",
  startedAtMs: 0,
};
const behind = (path: string, n: number, conflicts: string[] = []): Event => ({
  type: "worktreeFactsChanged",
  worktreePath: path,
  facts: { ahead: 2, behind: n, dirty: 0, conflicts, changedFiles: conflicts.length, commits: 2 },
});

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  return fake;
}

const sidebarRow = (branch: string) =>
  within(screen.getByRole("complementary", { name: "Repositories" })).getByText(branch).closest("li")!;
const card = (branch: string) => screen.getByRole("article", { name: `Worktree ${branch}` });
const openSession = () =>
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByText("fix/readme"));

test("a worktree behind its base shows ↓N on the sidebar row, the card and the session header", () => {
  launch({ type: "sessionStarted", session }, behind("/wt/acme/fix", 3), behind("/wt/acme/feat", 0));

  expect(sidebarRow("fix/readme").textContent).toContain("↓3");
  expect(sidebarRow("feat/login").textContent).not.toContain("↓");

  openSession();
  const header = screen.getByRole("region", { name: "Session fix/readme" }).querySelector(".lane-head")!;
  expect(header.textContent).toContain("↓3 behind main");

  fireEvent.click(screen.getByRole("button", { name: "Worktrees" }));
  expect(card("fix/readme").textContent).toContain("↓3");
});

test("Rebase on base is offered on a behind worktree's card only, and runs through the core", () => {
  const { sent } = launch(behind("/wt/acme/feat", 2, ["README"]), behind("/wt/acme/fix", 0));
  fireEvent.click(screen.getByRole("button", { name: "Worktrees" }));

  expect(within(card("fix/readme")).queryByRole("button", { name: "Rebase on main" })).toBeNull();
  expect(card("feat/login").textContent).toContain("conflicts with main");
  fireEvent.click(within(card("feat/login")).getByRole("button", { name: "Rebase on main" }));

  expect(sent).toContainEqual({ type: "rebaseWorktree", worktreePath: "/wt/acme/feat" });
});

test("a finished rebase is a result toast; one stopped on conflicts says how to go on", () => {
  const { emit } = launch(behind("/wt/acme/feat", 2));

  act(() => emit({ type: "worktreeRebased", worktreePath: "/wt/acme/feat", base: "main" }));
  expect(screen.getByRole("status").textContent).toContain("Rebased feat/login on main");

  act(() => emit({ type: "rebaseStopped", worktreePath: "/wt/acme/feat", base: "main", conflicts: ["README"] }));
  expect(screen.getByRole("status").textContent).toContain(
    "Rebasing feat/login on main stopped on conflicts in README: resolve them in Changes, then Rebase again to continue",
  );
});

test("History lists the branch's commits above the base marker and offers Rebase when behind", () => {
  const { sent, emit } = launch({ type: "sessionStarted", session: { ...session, status: "done" } }, behind("/wt/acme/fix", 1));
  openSession();

  fireEvent.click(screen.getByRole("tab", { name: /History/ }));
  expect(sent).toContainEqual({ type: "readHistory", worktreePath: "/wt/acme/fix" });
  act(() =>
    emit({
      type: "worktreeHistory",
      worktreePath: "/wt/acme/fix",
      base: "main",
      commits: [
        { hash: "b".repeat(40), short: "bbbbbbb", subject: "Fix the link", author: "Ada", atMs: 2000 },
        { hash: "a".repeat(40), short: "aaaaaaa", subject: "Reword intro", author: "Ada", atMs: 1000 },
      ],
    }),
  );

  const history = screen.getByRole("list", { name: "Commits on fix/readme" });
  const rows = within(history).getAllByRole("listitem");
  expect(rows.map((r) => r.textContent)).toEqual([
    expect.stringContaining("Fix the link"),
    expect.stringContaining("Reword intro"),
    expect.stringContaining("main"),
  ]);
  expect(rows[2].textContent).toContain("1 commit ahead of this branch");
  fireEvent.click(within(rows[2]).getByRole("button", { name: "Rebase" }));
  expect(sent).toContainEqual({ type: "rebaseWorktree", worktreePath: "/wt/acme/fix" });
  fireEvent.click(within(rows[2]).getByRole("button", { name: "Ask agent to rebase" }));
  expect(sent).toContainEqual({ type: "askAgentToRebase", sessionId: "session-1" });
});

test("History with no commits shows only the base marker", () => {
  const { emit } = launch({ type: "sessionStarted", session }, behind("/wt/acme/fix", 0));
  openSession();
  fireEvent.click(screen.getByRole("tab", { name: /History/ }));
  act(() => emit({ type: "worktreeHistory", worktreePath: "/wt/acme/fix", base: "main", commits: [] }));

  const rows = within(screen.getByRole("list", { name: "Commits on fix/readme" })).getAllByRole("listitem");
  expect(rows).toHaveLength(1);
  expect(rows[0].textContent).toContain("up to date");
  expect(within(rows[0]).queryByRole("button", { name: "Rebase" })).toBeNull();
});

test("History re-reads when the core signals git changed", () => {
  const { sent, emit } = launch({ type: "sessionStarted", session }, behind("/wt/acme/fix", 1));
  openSession();
  fireEvent.click(screen.getByRole("tab", { name: /History/ }));
  const reads = () => sent.filter((c) => c.type === "readHistory").length;
  const before = reads();

  act(() => emit({ type: "worktreeChanged", worktreePath: "/wt/acme/fix", kinds: ["git"] }));

  expect(reads()).toBe(before + 1);
});

test("Context shows where the branch stands with a rebase action when behind", () => {
  const { sent } = launch({ type: "sessionStarted", session: { ...session, status: "idle" } }, behind("/wt/acme/fix", 4));
  openSession();

  fireEvent.click(screen.getByRole("tab", { name: "Context" }));
  const branch = screen.getByRole("group", { name: "Branch fix/readme" });
  expect(branch.textContent).toContain("↑2");
  expect(branch.textContent).toContain("↓4");
  fireEvent.click(within(branch).getByRole("button", { name: "Rebase on main" }));

  expect(sent).toContainEqual({ type: "rebaseWorktree", worktreePath: "/wt/acme/fix" });
});

test("Rebase is a mechanical action: not while the session is Running or Needs you", () => {
  const { emit } = launch({ type: "sessionStarted", session }, behind("/wt/acme/fix", 1));
  openSession();
  fireEvent.click(screen.getByRole("tab", { name: /History/ }));
  act(() => emit({ type: "worktreeHistory", worktreePath: "/wt/acme/fix", base: "main", commits: [] }));
  const rebaseButton = () =>
    within(screen.getByRole("list", { name: "Commits on fix/readme" })).getByRole("button", { name: "Rebase" }) as HTMLButtonElement;
  expect(rebaseButton().disabled).toBe(true);
  expect(rebaseButton().title).toContain("Paused, Finished or Error");
  act(() => emit({ type: "sessionStatusChanged", sessionId: "session-1", status: "idle" }));
  expect(rebaseButton().disabled).toBe(false);
});

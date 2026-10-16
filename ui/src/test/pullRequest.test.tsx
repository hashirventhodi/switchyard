import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, onTestFinished, test, vi } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { PullRequest } from "../schema/PullRequest";
import type { Repository } from "../schema/Repository";
import type { Session } from "../schema/Session";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme: Repository = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: "origin" };
const repoEvents = (repository: Repository = acme): Event[] => [
  { type: "attached" },
  { type: "repositoryAdded", repository },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/login", branch: "feat/login", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/docs", branch: "docs/api", primary: false } },
];
const finished: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/login",
  branch: "feat/login",
  agent: "claudeCode",
  permissionMode: "autoEdit",
  model: { type: "default" },
  status: "done",
  startedAtMs: 0,
};
const pr = (change: Partial<PullRequest> = {}): PullRequest => ({
  number: 421,
  url: "https://github.com/acme/acme/pull/421",
  state: "open",
  draft: false,
  checks: "none",
  checkRuns: [],
  review: "none",
  ...change,
});
const prChanged = (pullRequest: PullRequest | null, worktreePath = "/wt/acme/login", checkedAtMs = 0): Event => ({
  type: "pullRequestChanged",
  worktreePath,
  pullRequest,
  checkedAtMs,
});

function launch(events: Event[], repository?: Repository) {
  const fake = fakeCore();
  const openLink = vi.fn();
  render(<App core={fake.core} pickFolder={async () => null} openLink={openLink} />);
  act(() => fake.emit(...repoEvents(repository), ...events));
  return { ...fake, openLink };
}

const sidebar = () => screen.getByRole("complementary", { name: "Repositories" });
const sidebarRow = (branch: string) => within(sidebar()).getByText(branch).closest("li")!;
const openSession = () => fireEvent.click(within(sidebar()).getByText("feat/login"));
const header = () => screen.getByRole("region", { name: "Session feat/login" }).querySelector(".lane-head")! as HTMLElement;
const finishedBox = () => screen.getByRole("group", { name: "Finished" });
const card = (branch: string) => screen.getByRole("article", { name: `Worktree ${branch}` });
const toasts = () => Array.from(document.querySelectorAll(".toast .tx")).map((t) => t.textContent ?? "");

test("the Dashboard shows gh missing or signed out as a setup state, and Check again re-detects it", () => {
  const { sent, openLink } = launch([
    {
      type: "forgeChecked",
      forge: {
        name: "GitHub",
        state: { type: "missing", installHint: "brew install gh", link: { label: "Install GitHub CLI", url: "https://cli.github.com" } },
      },
    },
  ]);
  const setup = screen.getByRole("region", { name: "Setup" });
  expect(within(setup).getByText("GitHub CLI (gh) is not installed")).toBeTruthy();
  expect(within(setup).getByText("brew install gh")).toBeTruthy();
  fireEvent.click(within(setup).getByRole("button", { name: "Install GitHub CLI" }));
  expect(openLink).toHaveBeenCalledWith("https://cli.github.com");
  fireEvent.click(within(setup).getByRole("button", { name: "Check again" }));
  expect(sent).toContainEqual({ type: "checkForge" });

  cleanup();
  launch([{ type: "forgeChecked", forge: { name: "GitHub", state: { type: "signedOut", loginHint: "gh auth login" } } }]);
  const again = screen.getByRole("region", { name: "Setup" });
  expect(again.textContent).toContain("GitHub CLI (gh) is not signed in");
  expect(within(again).getByText("gh auth login")).toBeTruthy();

  cleanup();
  launch([{ type: "forgeChecked", forge: { name: "GitHub", state: { type: "ready", version: "2.81.0" } } }]);
  expect(screen.queryByRole("region", { name: "Setup" })).toBeNull();
});

test("Open pull request on the session header and the Finished box goes through the core", () => {
  const { sent } = launch([{ type: "sessionStarted", session: finished }]);
  openSession();

  fireEvent.click(within(header()).getByRole("button", { name: "Open pull request" }));
  fireEvent.click(within(finishedBox()).getByRole("button", { name: "Open pull request" }));
  expect(sent.filter((c) => c.type === "openPullRequest")).toEqual([
    { type: "openPullRequest", worktreePath: "/wt/acme/login" },
    { type: "openPullRequest", worktreePath: "/wt/acme/login" },
  ]);
  expect(within(header()).queryByRole("button", { name: "Local merge" })).toBeNull();
});

test("a refusal shows as a toast naming the reason", () => {
  const { emit } = launch([{ type: "sessionStarted", session: finished }]);
  act(() =>
    emit({
      type: "commandRefused",
      command: "openPullRequest",
      reason: "feat/login has 2 uncommitted files; commit them before opening a pull request",
    }),
  );
  expect(toasts().join("\n")).toContain("feat/login has 2 uncommitted files");
});

test("the pull request's substate shows on the header, the session card, the worktree card and the sidebar row", () => {
  const { emit, openLink } = launch([
    { type: "sessionStarted", session: finished },
    { type: "pullRequestOpened", worktreePath: "/wt/acme/login", pullRequest: pr() },
    prChanged(pr()),
    prChanged(pr({ checks: "running" })),
  ]);
  expect(toasts().join("\n")).toContain("Opened PR #421 for feat/login");
  const text = "PR #421 open · checks running";

  expect(sidebarRow("feat/login").textContent).toContain(text);
  expect(screen.getByRole("button", { name: "Open feat/login" }).closest(".card")!.textContent).toContain(text);

  openSession();
  expect(header().textContent).toContain(text);
  // An open pull request is viewed, not opened again.
  expect(within(header()).queryByRole("button", { name: "Open pull request" })).toBeNull();
  fireEvent.click(within(header()).getByRole("button", { name: "View PR #421" }));
  expect(openLink).toHaveBeenCalledWith("https://github.com/acme/acme/pull/421");

  act(() => emit(prChanged(pr({ checks: "failing" }))));
  expect(header().textContent).toContain("PR #421 open · checks failing");
  act(() => emit(prChanged(pr({ checks: "passing", review: "approved" }))));
  expect(header().textContent).toContain("PR #421 open · checks passing · approved");

  fireEvent.click(screen.getByRole("button", { name: "Worktrees" }));
  expect(card("feat/login").textContent).toContain("PR #421 open · checks passing · approved");
  expect(card("docs/api").textContent).not.toContain("PR #");
});

test("Merged cleanup is confirmed by one toast; a kept worktree says why", () => {
  const { emit } = launch([{ type: "sessionStarted", session: finished }, prChanged(pr())]);
  act(() =>
    emit(
      prChanged(pr({ state: "merged" })),
      { type: "worktreeRemoved", repositoryId: "repo-1", worktreePath: "/wt/acme/login" },
      {
        type: "branchMerged",
        repositoryId: "repo-1",
        worktreePath: "/wt/acme/login",
        branch: "feat/login",
        base: "main",
        pullRequest: 421,
        removed: true,
        kept: null,
      },
    ),
  );
  expect(toasts()).toEqual(["PR #421 merged · removed worktree feat/login"]);
  expect(within(sidebar()).queryByText("feat/login")).toBeNull();

  act(() =>
    emit(prChanged(pr({ number: 9, state: "merged" }), "/wt/acme/docs"), {
      type: "branchMerged",
      repositoryId: "repo-1",
      worktreePath: "/wt/acme/docs",
      branch: "docs/api",
      base: "main",
      pullRequest: 9,
      removed: false,
      kept: "docs/api has 1 uncommitted file; commit or discard it before removing the worktree",
    }),
  );
  expect(toasts()[1]).toContain("PR #9 merged · kept docs/api: docs/api has 1 uncommitted file");
  expect(sidebarRow("docs/api").textContent).toContain("PR #9 merged");
});

test("Prune merged runs per repository and reports what it removed", () => {
  const { sent, emit } = launch([]);
  fireEvent.click(screen.getByRole("button", { name: "Worktrees" }));
  const section = screen.getByRole("region", { name: "acme worktrees" });
  fireEvent.click(within(section).getByRole("button", { name: "Prune merged" }));
  expect(sent).toContainEqual({ type: "pruneMerged", repositoryId: "repo-1" });

  act(() =>
    emit(
      { type: "worktreeRemoved", repositoryId: "repo-1", worktreePath: "/wt/acme/login" },
      { type: "worktreeRemoved", repositoryId: "repo-1", worktreePath: "/wt/acme/docs" },
      { type: "mergedPruned", repositoryId: "repo-1", branches: ["docs/api", "feat/login"] },
    ),
  );
  expect(toasts()).toEqual(["Pruned 2 merged worktrees: docs/api, feat/login"]);
  act(() => emit({ type: "mergedPruned", repositoryId: "repo-1", branches: [] }));
  expect(toasts()[1]).toBe("No merged worktrees to prune");
});

test("a repository with no remote gets Local merge instead, and never both", () => {
  const lone: Repository = { ...acme, remote: null };
  const { sent, emit } = launch([{ type: "sessionStarted", session: finished }], lone);
  openSession();
  expect(within(header()).queryByRole("button", { name: "Open pull request" })).toBeNull();
  fireEvent.click(within(finishedBox()).getByRole("button", { name: "Local merge into main" }));
  expect(sent).toContainEqual({ type: "localMerge", worktreePath: "/wt/acme/login" });

  fireEvent.click(screen.getByRole("button", { name: "Worktrees" }));
  expect(within(screen.getByRole("region", { name: "acme worktrees" })).queryByRole("button", { name: "Prune merged" })).toBeNull();
  expect(within(card("docs/api")).getByRole("button", { name: "Local merge into main" })).toBeTruthy();
  expect(within(card("main")).queryByRole("button", { name: "Local merge into main" })).toBeNull();

  act(() =>
    emit({
      type: "branchMerged",
      repositoryId: "repo-1",
      worktreePath: "/wt/acme/docs",
      branch: "docs/api",
      base: "main",
      pullRequest: null,
      removed: true,
      kept: null,
    }),
  );
  expect(toasts()).toContain("Merged docs/api into main · removed worktree docs/api");

  // With a remote: Open pull request on cards, never Local merge.
  cleanup();
  launch([]);
  fireEvent.click(screen.getByRole("button", { name: "Worktrees" }));
  expect(within(card("docs/api")).queryByRole("button", { name: "Local merge into main" })).toBeNull();
  expect(within(card("docs/api")).getByRole("button", { name: "Open pull request" })).toBeTruthy();
});

test("History shows the pull request beneath the commits with each check and its status", () => {
  const { emit, openLink } = launch([{ type: "sessionStarted", session: finished }]);
  openSession();
  fireEvent.click(screen.getByRole("tab", { name: /History/ }));
  act(() =>
    emit(
      {
        type: "worktreeHistory",
        worktreePath: "/wt/acme/login",
        base: "main",
        commits: [{ hash: "a".repeat(40), short: "aaaaaaa", subject: "Add the login form", author: "Ada", atMs: 1000 }],
      },
      prChanged(
        pr({
          checks: "failing",
          checkRuns: [
            { name: "build", status: "passing", url: "https://github.com/acme/acme/runs/1" },
            { name: "test", status: "failing", url: null },
            { name: "deploy", status: "pending", url: null },
          ],
        }),
      ),
    ),
  );

  const section = screen.getByRole("region", { name: "Pull request" });
  const commits = screen.getByRole("list", { name: "Commits on feat/login" });
  expect(section.compareDocumentPosition(commits) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();
  expect(section.textContent).toContain("#421");
  expect(section.textContent).toContain("open");
  const checks = within(within(section).getByRole("list", { name: "Checks" })).getAllByRole("listitem");
  expect(checks.map((c) => c.textContent)).toEqual([
    expect.stringMatching(/build.*passing/),
    expect.stringMatching(/test.*failing/),
    expect.stringMatching(/deploy.*pending/),
  ]);
  fireEvent.click(within(checks[0]).getByRole("button", { name: "build" }));
  expect(openLink).toHaveBeenCalledWith("https://github.com/acme/acme/runs/1");
  expect(within(checks[1]).queryByRole("button")).toBeNull();
  fireEvent.click(within(section).getByRole("button", { name: "View PR #421" }));
  expect(openLink).toHaveBeenCalledWith("https://github.com/acme/acme/pull/421");

  act(() => emit(prChanged(pr({ state: "merged", checks: "passing", checkRuns: [] }))));
  expect(screen.getByRole("region", { name: "Pull request" }).textContent).toContain("merged");
  expect(screen.getByRole("region", { name: "Pull request" }).textContent).toContain("No checks reported");
});

test("History shows no pull request section when the branch has none", () => {
  const { emit } = launch([{ type: "sessionStarted", session: finished }]);
  openSession();
  fireEvent.click(screen.getByRole("tab", { name: /History/ }));
  act(() => emit({ type: "worktreeHistory", worktreePath: "/wt/acme/login", base: "main", commits: [] }));
  expect(screen.queryByRole("region", { name: "Pull request" })).toBeNull();
});

test("the pull request card says how long ago the Forge was checked; no local view does", () => {
  const NOW = 1_760_000_000_000;
  vi.useFakeTimers({ now: NOW });
  onTestFinished(() => {
    vi.useRealTimers();
  });
  const { emit } = launch([{ type: "sessionStarted", session: finished }, prChanged(pr(), "/wt/acme/login", NOW - 12_000)]);
  openSession();
  fireEvent.click(screen.getByRole("tab", { name: /History/ }));
  act(() => emit({ type: "worktreeHistory", worktreePath: "/wt/acme/login", base: "main", commits: [] }));
  const section = () => screen.getByRole("region", { name: "Pull request" });
  expect(section().textContent).toContain("checked 12 s ago");
  act(() => vi.advanceTimersByTime(5_000));
  expect(section().textContent).toContain("checked 17 s ago");
  act(() => emit(prChanged(pr(), "/wt/acme/login", NOW + 4_000)));
  expect(section().textContent).toContain("checked 1 s ago");
  // Only the pull request card: not the header, the sidebar, nor anything local.
  expect(screen.getAllByText(/checked \d+ s ago/)).toHaveLength(1);
  expect(header().textContent).not.toContain("checked");
  expect(sidebar().textContent).not.toContain("checked");
});

test("the checked-ago label stops repainting while the window is hidden and catches up when shown", () => {
  const NOW = 1_760_000_000_000;
  vi.useFakeTimers({ now: NOW });
  const setVisibility = (state: "visible" | "hidden") => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state });
    act(() => {
      document.dispatchEvent(new window.Event("visibilitychange"));
    });
  };
  onTestFinished(() => {
    setVisibility("visible");
    vi.useRealTimers();
  });
  const { emit } = launch([{ type: "sessionStarted", session: finished }, prChanged(pr(), "/wt/acme/login", NOW - 12_000)]);
  openSession();
  fireEvent.click(screen.getByRole("tab", { name: /History/ }));
  act(() => emit({ type: "worktreeHistory", worktreePath: "/wt/acme/login", base: "main", commits: [] }));
  const section = () => screen.getByRole("region", { name: "Pull request" });

  setVisibility("hidden");
  act(() => vi.advanceTimersByTime(30_000));
  expect(section().textContent).toContain("checked 12 s ago");

  setVisibility("visible");
  expect(section().textContent).toContain("checked 42 s ago");
});

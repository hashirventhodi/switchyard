// Worktrees added, removed or renamed in a shell are pushed by the core (ADR-0008): the
// sidebar gains, renames and loses cards from those events alone, with no view opened.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import type { SessionStatus } from "../schema/SessionStatus";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: null };
const login = { path: "/wt/login", branch: "feat/login", primary: false };

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() =>
    fake.emit(
      { type: "attached" },
      { type: "repositoryAdded", repository: acme },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
      ...events,
    ),
  );
  return fake;
}

const sidebar = () => screen.getByRole("complementary", { name: "Repositories" });
const cards = () => within(sidebar()).getAllByRole("listitem").map((li) => li.querySelector(".branch")?.firstChild?.textContent);

const session = (id: string, worktreePath: string, branch: string, status: SessionStatus): Session => ({
  id,
  repositoryId: "repo-1",
  worktreePath,
  branch,
  agent: "claudeCode",
  permissionMode: "askFirst",
  model: { type: "default" },
  status,
  startedAtMs: 1_760_000_000_000,
});

test("a worktree added in a shell gains a sidebar card, keeps its place when renamed, and loses it when removed", () => {
  const { emit } = launch({ type: "worktreeAdded", repositoryId: "repo-1", worktree: login });
  act(() => emit({ type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/cache", branch: "spike/cache", primary: false } }));
  expect(cards()).toEqual(["main", "feat/login", "spike/cache"]);

  act(() => emit({ type: "worktreeAdded", repositoryId: "repo-1", worktree: { ...login, branch: "feat/sign-in" } }));
  expect(cards()).toEqual(["main", "feat/sign-in", "spike/cache"]);

  act(() => emit({ type: "worktreeRemoved", repositoryId: "repo-1", worktreePath: "/wt/login" }));
  expect(cards()).toEqual(["main", "spike/cache"]);
});

test("a session stopped and archived because its worktree was removed in a shell leaves the sidebar and shows under Archived", () => {
  const { emit } = launch(
    { type: "worktreeAdded", repositoryId: "repo-1", worktree: login },
    { type: "sessionStarted", session: session("session-1", "/wt/login", "feat/login", "running") },
    { type: "sessionStarted", session: session("session-2", "/src/acme", "main", "idle") },
  );

  act(() =>
    emit(
      { type: "sessionStatusChanged", sessionId: "session-1", status: "idle" },
      { type: "sessionArchived", archived: { session: session("session-1", "/wt/login", "feat/login", "idle"), archivedAtMs: 1_760_000_100_000, entries: 3 } },
      { type: "worktreeRemoved", repositoryId: "repo-1", worktreePath: "/wt/login" },
    ),
  );

  expect(cards()).toEqual(["main"]);
  fireEvent.click(within(sidebar()).getByRole("button", { name: /^main/ }));
  fireEvent.click(screen.getByRole("tab", { name: /Context/ }));
  const row = within(screen.getByRole("region", { name: "Archived sessions" })).getByRole("listitem");
  expect(row.textContent).toMatch(/feat\/login/);
  expect(row.textContent).toMatch(/session-1/);
});

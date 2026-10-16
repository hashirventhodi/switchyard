// A Worktree whose watcher cannot run is re-derived slowly by the core, and the renderer
// says so: a notice per Worktree from `worktreeUnwatched`, gone on `worktreeWatched`.
import { act, cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: null };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/feat", branch: "feat/login", primary: false } },
];
const unwatched: Event = { type: "worktreeUnwatched", worktreePath: "/wt/feat", reason: "inotify watch limit reached" };

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  return fake;
}

const sidebar = () => screen.getByRole("complementary", { name: "Repositories" });
const worktreeRow = (path: string) => within(sidebar()).getAllByTitle(path).find((e) => e.tagName === "LI")!;

test("an unwatched worktree shows a notice naming it and the reason", () => {
  launch(unwatched);

  const notice = within(worktreeRow("/wt/feat")).getByRole("status");
  expect(notice.textContent).toBe("Live updates unavailable for feat/login: inotify watch limit reached");
  expect(within(worktreeRow("/src/acme")).queryByRole("status")).toBeNull();
});

test("the notice clears on the all-clear and nothing else about the worktree changes", () => {
  const { emit } = launch();
  const before = worktreeRow("/wt/feat").innerHTML;

  act(() => emit(unwatched));
  expect(within(worktreeRow("/wt/feat")).getByRole("status")).toBeTruthy();

  act(() => emit({ type: "worktreeWatched", worktreePath: "/wt/feat" }));
  expect(within(worktreeRow("/wt/feat")).queryByRole("status")).toBeNull();
  expect(worktreeRow("/wt/feat").innerHTML).toBe(before);
});

test("a new attach forgets notices until the core replays them", () => {
  const { emit } = launch(unwatched);

  act(() => emit(...repoEvents));

  expect(within(worktreeRow("/wt/feat")).queryByRole("status")).toBeNull();
});

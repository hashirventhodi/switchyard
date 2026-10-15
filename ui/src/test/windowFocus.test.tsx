// Window focus and visibility do one thing each (ADR-0008): focus asks the core to poll the
// Forge now; visibility tells it how often to poll while nobody is looking. Git is watched
// by the core, so neither re-reads it, and the renderer runs no data timer of its own.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: "origin" };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/fix", branch: "fix/readme", primary: false } },
];
const running: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  status: "running",
  startedAtMs: 0,
};

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  return fake;
}

function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state });
  act(() => {
    document.dispatchEvent(new window.Event("visibilitychange"));
  });
}

test("returning to the window polls the Forge now and does not re-read git", () => {
  const { sent } = launch();
  const before = sent.length;

  act(() => {
    window.dispatchEvent(new window.Event("focus"));
  });

  expect(sent.slice(before)).toEqual([{ type: "pollForgeNow" }]);
});

test("hiding and showing the window tells the core", () => {
  const { sent } = launch();
  const before = sent.length;

  setVisibility("hidden");
  setVisibility("visible");

  expect(sent.slice(before)).toEqual([
    { type: "windowVisibility", visible: false },
    { type: "windowVisibility", visible: true },
  ]);
});

test("an hour passes with a session running and a pull request open, and nothing is asked of the core", () => {
  vi.useFakeTimers();
  const { sent } = launch(
    { type: "sessionStarted", session: running },
    {
      type: "pullRequestChanged",
      worktreePath: "/wt/fix",
      pullRequest: {
        number: 7,
        url: "https://github.com/acme/acme/pull/7",
        state: "open",
        draft: false,
        checks: "none",
        checkRuns: [],
        review: "none",
      },
      checkedAtMs: 0,
    },
  );
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByText("fix/readme"));
  const before = sent.length;

  act(() => {
    vi.advanceTimersByTime(60 * 60 * 1000);
  });

  expect(sent.slice(before)).toEqual([]);
});

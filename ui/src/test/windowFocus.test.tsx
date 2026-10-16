// Window focus and visibility only steer the Forge poller (ADR-0008): the renderer tells the
// core whether anyone is looking (shown and focused), from startup on, so a hidden window is
// polled at the slowest cadence; returning to the window polls the Forge once, at once. Git
// is watched by the core, so nothing re-reads it, and the renderer runs no data timer.
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

afterEach(() => setVisibility("visible"));

function windowEvent(type: "focus" | "blur") {
  act(() => {
    window.dispatchEvent(new window.Event(type));
  });
}

test("at startup the core is told the window is visible", () => {
  const { sent } = launch();

  expect(sent.filter((c) => c.type === "windowVisibility")).toEqual([{ type: "windowVisibility", visible: true }]);
});

test("focus on a window the core already counts as visible polls the Forge now and does not re-read git", () => {
  const { sent } = launch();
  const before = sent.length;

  windowEvent("focus");

  expect(sent.slice(before)).toEqual([{ type: "pollForgeNow" }]);
});

test("leaving the window and returning tells the core each once, which is the one Forge poll", () => {
  const { sent } = launch();
  const before = sent.length;

  windowEvent("blur");
  setVisibility("hidden");
  setVisibility("visible");
  windowEvent("focus");

  // The core polls when it is told it is visible again after being hidden.
  expect(sent.slice(before)).toEqual([
    { type: "windowVisibility", visible: false },
    { type: "windowVisibility", visible: true },
  ]);
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
  const { sent, emit } = launch(
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
  fireEvent.click(screen.getByRole("tab", { name: /History/ }));
  act(() => emit({ type: "worktreeHistory", worktreePath: "/wt/fix", base: "main", commits: [] }));
  const before = sent.length;

  // The only timers left are cosmetic: CheckedAgo repaints "checked N s ago" on the pull
  // request card (on screen here); it sends nothing.
  expect(screen.getByText(/^checked \d+ (s|min) ago$/)).toBeTruthy();
  act(() => {
    vi.advanceTimersByTime(60 * 60 * 1000);
  });

  expect(sent.slice(before)).toEqual([]);
});

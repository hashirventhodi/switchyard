// A Session's Model in the renderer: New session's Model field (Default, or a Model id
// typed in), and the summary strip, which never hides what a Default resolved to.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { ModelChoice } from "../schema/ModelChoice";
import type { Session } from "../schema/Session";
import type { SessionFacts } from "../schema/SessionFacts";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/fix", branch: "fix/readme", primary: false } },
];
const session = (model: ModelChoice): Session => ({
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  model,
  status: "running",
  startedAtMs: 0,
});
const facts = (more: Partial<SessionFacts>): Event => ({
  type: "sessionFactsChanged",
  sessionId: "session-1",
  facts: { model: null, cliVersion: null, cliPermissionMode: null, turns: 0, modeWarning: null, classifierDenials: 0, mcpServers: [], ...more },
});

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  return fake;
}

function newSession() {
  fireEvent.click(screen.getByRole("button", { name: "New session" }));
  return within(screen.getByRole("dialog", { name: "New session" }));
}

function start(dialog: ReturnType<typeof newSession>) {
  fireEvent.change(dialog.getByLabelText("Prompt"), { target: { value: "Fix the README" } });
  fireEvent.click(dialog.getByRole("button", { name: /^Start/ }));
}

test("New session starts on Default unless a Model is typed", () => {
  const { sent } = launch();
  const dialog = newSession();

  expect((dialog.getByLabelText("Model") as HTMLInputElement).value).toBe("");
  expect(dialog.getByLabelText("Model").getAttribute("placeholder")).toBe("Default");
  start(dialog);

  expect(sent.find((c) => c.type === "startSessionOnNewWorktree")).toMatchObject({ model: { type: "default" } });
});

test("New session starts on the Model typed into the Model field", () => {
  const { sent } = launch();
  const dialog = newSession();

  fireEvent.change(dialog.getByLabelText("Model"), { target: { value: "  claude-sonnet-5-5 " } });
  start(dialog);

  expect(sent.find((c) => c.type === "startSessionOnNewWorktree")).toMatchObject({
    model: { type: "model", id: "claude-sonnet-5-5" },
  });
});

test("an Explore session gets the same Model field", () => {
  const { sent } = launch();
  const dialog = newSession();

  fireEvent.click(dialog.getByRole("radio", { name: "Explore" }));
  fireEvent.change(dialog.getByLabelText("Model"), { target: { value: "claude-haiku-4-5" } });
  start(dialog);

  expect(sent.find((c) => c.type === "startSessionOnNewWorktree")).toMatchObject({
    permissionMode: "explore",
    model: { type: "model", id: "claude-haiku-4-5" },
  });
});

test("a Codex Session starts on the Model chosen for it", () => {
  const { sent } = launch();
  const dialog = newSession();

  fireEvent.change(dialog.getByLabelText("Agent"), { target: { value: "codex" } });
  fireEvent.change(dialog.getByLabelText("Model"), { target: { value: "gpt-5.6-sol" } });
  start(dialog);

  expect(sent.find((c) => c.type === "startSessionOnNewWorktree")).toMatchObject({
    agent: "codex",
    model: { type: "model", id: "gpt-5.6-sol" },
  });
});

const modelCell = () => {
  const label = within(screen.getByLabelText("Summary")).getByText("Model", { selector: ".l" });
  return label.nextElementSibling?.textContent;
};
const openSession = () =>
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));

test("the summary strip shows a chosen Model as the CLI reports it", () => {
  const { emit } = launch({ type: "sessionStarted", session: session({ type: "model", id: "sonnet" }) });
  openSession();
  expect(modelCell()).toBe("sonnet");

  act(() => emit(facts({ model: "claude-sonnet-5-5", cliVersion: "2.1.300" })));
  expect(modelCell()).toBe("claude-sonnet-5-5");
});

test("the summary strip shows what Default resolved to, and never a guess", () => {
  const { emit } = launch({ type: "sessionStarted", session: session({ type: "default" }) });
  openSession();
  // Not started yet: nothing to report.
  expect(modelCell()).toBe("Default");

  act(() => emit(facts({ model: null, cliVersion: "2.1.300" })));
  expect(modelCell()).toBe("Default (not reported)");

  act(() => emit(facts({ model: "claude-opus-5-5", cliVersion: "2.1.300" })));
  expect(modelCell()).toBe("Default → claude-opus-5-5");
});

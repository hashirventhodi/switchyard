// The composer's Model control: it switches a running Session's Model, shows the switch as
// pending until the core confirms it, and on failure shows the CLI's reason and goes back to
// the Model actually running. Explore sessions have it too.
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
];
const session: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  model: { type: "default" },
  status: "running",
  startedAtMs: 0,
};
const runningOn = (model: string): Event => ({
  type: "sessionFactsChanged",
  sessionId: "session-1",
  facts: { model, cliVersion: "2.1.300", cliPermissionMode: "default", turns: 0, modeWarning: null, classifierDenials: 0, mcpServers: [] },
});
const switchState = (pendingModel: string | null, switchedModel: string | null = null): Event => ({
  type: "sessionModelChanged",
  sessionId: "session-1",
  pendingModel,
  switchedModel,
});

const checked = (switchModelLive: boolean): Event => ({
  type: "agentsChecked",
  agents: [{ agent: "claudeCode", name: "Claude Code", state: { type: "ready", version: "2.1.300" }, switchModelLive }],
});

/** The app with a Session on Claude Code, which switches live unless `events` say otherwise. */
function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(checked(true), ...repoEvents, ...events));
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  return fake;
}

const control = () => screen.getByRole("group", { name: "Session Model" });
const field = () => within(control()).getByRole("textbox", { name: "Model" }) as HTMLInputElement;

function switchTo(model: string) {
  fireEvent.change(field(), { target: { value: model } });
  fireEvent.click(within(control()).getByRole("button", { name: "Switch" }));
}

test("the composer's Model control switches the running Session's Model", () => {
  const { sent } = launch({ type: "sessionStarted", session }, runningOn("claude-opus-5-5"));
  expect(field().value).toBe("claude-opus-5-5");

  switchTo("claude-sonnet-5-5");

  expect(sent).toContainEqual({ type: "setModel", sessionId: "session-1", model: "claude-sonnet-5-5" });
});

test("a switch shows as pending until it is confirmed", () => {
  const { emit } = launch({ type: "sessionStarted", session }, runningOn("claude-opus-5-5"));

  act(() => emit(switchState("claude-sonnet-5-5")));
  expect(within(control()).getByRole("status").textContent).toBe("Switching to claude-sonnet-5-5 from the agent's next request…");
  expect(field().value).toBe("claude-sonnet-5-5");

  act(() => emit(switchState(null, "claude-sonnet-5-5"), runningOn("claude-sonnet-5-5")));
  expect(within(control()).queryByRole("status")).toBeNull();
  expect(field().value).toBe("claude-sonnet-5-5");
});

test("a failed switch shows the CLI's reason and goes back to the Model running", () => {
  const { emit } = launch({ type: "sessionStarted", session }, runningOn("claude-opus-5-5"), switchState("claude-nope"));

  act(() =>
    emit(switchState(null), { type: "modelSwitchFailed", sessionId: "session-1", model: "claude-nope", reason: "Model 'claude-nope' not found" }),
  );

  expect(within(control()).getByRole("alert").textContent).toBe(
    "Could not switch to claude-nope: Model 'claude-nope' not found.",
  );
  expect(field().value).toBe("claude-opus-5-5");
});

test("a switch that did not take says why, as the core words it", () => {
  const { emit } = launch({ type: "sessionStarted", session }, runningOn("claude-opus-5-5"), switchState("claude-sonnet-5-5"));
  const reason = "the agent's next request ran on claude-haiku-4-5";

  act(() => emit(switchState(null), { type: "modelSwitchFailed", sessionId: "session-1", model: "claude-sonnet-5-5", reason }));

  expect(within(control()).getByRole("alert").textContent).toBe(
    "Could not switch to claude-sonnet-5-5: the agent's next request ran on claude-haiku-4-5.",
  );
});

test("a new switch clears the last failure", () => {
  const { emit } = launch(
    { type: "sessionStarted", session },
    runningOn("claude-opus-5-5"),
    { type: "modelSwitchFailed", sessionId: "session-1", model: "claude-nope", reason: null },
  );

  act(() => emit(switchState("claude-sonnet-5-5")));

  expect(within(control()).queryByRole("alert")).toBeNull();
});

test("an Explore session switches Models too", () => {
  const { sent } = launch({ type: "sessionStarted", session: { ...session, permissionMode: "explore" } }, runningOn("claude-opus-5-5"));

  switchTo("claude-haiku-4-5");

  expect(sent).toContainEqual({ type: "setModel", sessionId: "session-1", model: "claude-haiku-4-5" });
});

test("on a CLI that cannot switch live, the control reads that a switch applies on resume", () => {
  const { sent, emit } = launch(checked(false), { type: "sessionStarted", session }, runningOn("claude-opus-5-5"));
  expect(within(control()).getByRole("note").textContent).toBe("Applies on resume");

  switchTo("claude-sonnet-5-5");
  expect(sent).toContainEqual({ type: "setModel", sessionId: "session-1", model: "claude-sonnet-5-5" });
  act(() => emit(switchState("claude-sonnet-5-5")));

  expect(within(control()).getByRole("status").textContent).toBe("Switches to claude-sonnet-5-5 when the session resumes.");
});

test("before the agents are checked, a switch is taken to apply on resume, as the core takes it", () => {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, { type: "sessionStarted", session }, runningOn("claude-opus-5-5")));
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));

  expect(within(control()).getByRole("note").textContent).toBe("Applies on resume");
});

test("on a CLI that switches live, there is no such note", () => {
  launch(checked(true), { type: "sessionStarted", session }, runningOn("claude-opus-5-5"));

  expect(within(control()).queryByRole("note")).toBeNull();
});

test("a Codex Session's control reads that a switch applies on resume", () => {
  const codexChecked: Event = {
    type: "agentsChecked",
    agents: [{ agent: "codex", name: "Codex", state: { type: "ready", version: "0.144.1" }, switchModelLive: false }],
  };
  const codexSession: Session = { ...session, agent: "codex" };
  const { sent } = launch(codexChecked, { type: "sessionStarted", session: codexSession }, runningOn("gpt-5.6-terra"));

  expect(within(control()).getByRole("note").textContent).toBe("Applies on resume");
  switchTo("gpt-5.6-sol");

  expect(sent).toContainEqual({ type: "setModel", sessionId: "session-1", model: "gpt-5.6-sol" });
});

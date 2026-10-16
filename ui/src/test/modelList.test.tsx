// New session's Model picker on the core's cached Model list: Default first, then the
// Models the CLI listed; free text with a note while there is no list (or the CLI cannot
// list), and a list that arrives late never replaces what the user typed.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { ModelList } from "../schema/ModelList";
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

const listed: ModelList = {
  type: "listed",
  models: [
    { id: "opus", displayName: "Opus" },
    { id: "sonnet", displayName: "Sonnet" },
  ],
  fetchedAtMs: 1_760_000_000_000,
};
const modelsChanged = (models: ModelList): Event => ({ type: "modelsChanged", agent: "claudeCode", models });

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

const startedModel = (sent: { type: string }[]) =>
  (sent.find((c) => c.type === "startSessionOnNewWorktree") as { model?: unknown } | undefined)?.model;

test("opening New session asks the core for the agent's Model list", () => {
  const { sent, emit } = launch();
  const dialog = newSession();
  act(() => emit(modelsChanged(listed)));
  // Once per opening (each one may ask a live CLI), not on every render.
  expect(sent.filter((c) => c.type === "readModels")).toEqual([{ type: "readModels", agent: "claudeCode" }]);

  fireEvent.change(dialog.getByLabelText("Agent"), { target: { value: "codex" } });
  expect(sent).toContainEqual({ type: "readModels", agent: "codex" });
});

test("a Codex Session picks from Codex's own cached list", () => {
  const codexListed: ModelList = {
    type: "listed",
    models: [
      { id: "gpt-5.6-terra", displayName: "GPT-5.6 Terra" },
      { id: "gpt-5.6-sol", displayName: "GPT-5.6 Sol" },
    ],
    fetchedAtMs: 1_760_000_000_000,
  };
  const { sent } = launch(modelsChanged(listed), { type: "modelsChanged", agent: "codex", models: codexListed });
  const dialog = newSession();
  fireEvent.change(dialog.getByLabelText("Agent"), { target: { value: "codex" } });

  const picker = dialog.getByLabelText("Model") as HTMLSelectElement;
  expect([...picker.options].map((o) => o.textContent)).toEqual(["Default", "GPT-5.6 Terra", "GPT-5.6 Sol"]);
  fireEvent.change(picker, { target: { value: "gpt-5.6-sol" } });
  start(dialog);

  expect(startedModel(sent)).toEqual({ type: "model", id: "gpt-5.6-sol" });
});

test("with no list yet, the Model is free text with a note that the list comes later", () => {
  launch(modelsChanged({ type: "empty" }));
  const dialog = newSession();

  expect(dialog.getByLabelText("Model").tagName).toBe("INPUT");
  expect(dialog.getByRole("note", { name: "Model list" }).textContent).toBe(
    "The Model list is loading from this CLI: type a Model id, or leave it blank for Default",
  );
});

test("a listed CLI offers Default first, then its Models", () => {
  const { sent } = launch(modelsChanged(listed));
  const dialog = newSession();

  const picker = dialog.getByLabelText("Model") as HTMLSelectElement;
  expect(picker.tagName).toBe("SELECT");
  expect([...picker.options].map((o) => o.textContent)).toEqual(["Default", "Opus", "Sonnet"]);
  expect(picker.value).toBe("");
  fireEvent.change(picker, { target: { value: "sonnet" } });
  start(dialog);

  expect(startedModel(sent)).toEqual({ type: "model", id: "sonnet" });
});

test("a list that arrives while the dialog is open fills the dropdown", () => {
  const { emit } = launch();
  const dialog = newSession();
  expect(dialog.getByLabelText("Model").tagName).toBe("INPUT");

  act(() => emit(modelsChanged(listed)));

  expect(dialog.getByLabelText("Model").tagName).toBe("SELECT");
  expect(dialog.queryByRole("note", { name: "Model list" })).toBeNull();
});

test("a list that arrives late never replaces a Model the user typed", () => {
  const { emit, sent } = launch();
  const dialog = newSession();
  fireEvent.change(dialog.getByLabelText("Model"), { target: { value: "claude-opus-5-5[1m]" } });

  act(() => emit(modelsChanged(listed)));

  expect((dialog.getByLabelText("Model") as HTMLInputElement).value).toBe("claude-opus-5-5[1m]");
  const beside = dialog.getByLabelText("Listed Models") as HTMLSelectElement;
  expect([...beside.options].map((o) => o.textContent)).toContain("Sonnet");
  start(dialog);
  expect(startedModel(sent)).toEqual({ type: "model", id: "claude-opus-5-5[1m]" });
});

test("picking from the list beside typed text takes the picked Model", () => {
  const { emit, sent } = launch();
  const dialog = newSession();
  fireEvent.change(dialog.getByLabelText("Model"), { target: { value: "opus-typo" } });
  act(() => emit(modelsChanged(listed)));

  fireEvent.change(dialog.getByLabelText("Listed Models"), { target: { value: "sonnet" } });
  start(dialog);

  expect(startedModel(sent)).toEqual({ type: "model", id: "sonnet" });
});

test("a CLI that cannot list its Models gets free text with a note saying so", () => {
  launch(modelsChanged({ type: "cannotList", reason: "timed out" }));
  const dialog = newSession();

  expect(dialog.getByLabelText("Model").tagName).toBe("INPUT");
  expect(dialog.getByRole("note", { name: "Model list" }).textContent).toBe(
    "This CLI did not list its Models (timed out): type a Model id, or leave it blank for Default",
  );
});

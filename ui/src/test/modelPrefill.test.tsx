// New session pre-fills the last Model choice made for the agent in the Repository, as the
// core remembers it: Default stays Default, a retired Model falls back to Default with a
// one-line note, and with no list cached the remembered id is offered as typed text.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { ModelChoice } from "../schema/ModelChoice";
import type { ModelList } from "../schema/ModelList";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const billing = { id: "repo-2", name: "billing", path: "/src/billing", baseBranch: "main" };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "repositoryAdded", repository: billing },
  { type: "worktreeAdded", repositoryId: "repo-2", worktree: { path: "/src/billing", branch: "main", primary: true } },
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
const prefill = (choice: ModelChoice, retired: string | null = null, repositoryId = "repo-1"): Event => ({
  type: "modelPrefill",
  agent: "claudeCode",
  repositoryId,
  choice,
  retired,
});
const sonnet: ModelChoice = { type: "model", id: "sonnet" };

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

test("New session asks the core for the agent's last Model choice in the Repository", () => {
  const { sent } = launch();
  const dialog = newSession();

  expect(sent).toContainEqual({ type: "readModelPrefill", agent: "claudeCode", repositoryId: "repo-1" });
  fireEvent.change(dialog.getByLabelText("Repository"), { target: { value: "repo-2" } });
  expect(sent).toContainEqual({ type: "readModelPrefill", agent: "claudeCode", repositoryId: "repo-2" });
});

test("the last choice is pre-filled and started with", () => {
  const { emit, sent } = launch(modelsChanged(listed));
  const dialog = newSession();

  act(() => emit(prefill(sonnet)));

  expect((dialog.getByLabelText("Model") as HTMLSelectElement).value).toBe("sonnet");
  start(dialog);
  expect(startedModel(sent)).toEqual(sonnet);
});

test("a remembered full id shows as the listed alias that stands for it, and starts on that id", () => {
  const aliases: ModelList = {
    type: "listed",
    models: [
      { id: "opus", displayName: "Opus", resolvedModel: "claude-opus-5-5" },
      { id: "sonnet", displayName: "Sonnet", resolvedModel: "claude-sonnet-5-5" },
    ],
    fetchedAtMs: 1_760_000_000_000,
  };
  const full: ModelChoice = { type: "model", id: "claude-sonnet-5-5" };
  const { emit, sent } = launch(modelsChanged(aliases));
  const dialog = newSession();

  act(() => emit(prefill(full)));

  const picker = dialog.getByLabelText("Model") as HTMLSelectElement;
  expect(picker.value).toBe("sonnet");
  expect([...picker.options].map((o) => o.textContent)).toEqual(["Default", "Opus", "Sonnet"]);
  start(dialog);
  expect(startedModel(sent)).toEqual(full);
});

test("another Repository's last choice is not pre-filled", () => {
  const { emit } = launch(modelsChanged(listed));
  const dialog = newSession();

  act(() => emit(prefill(sonnet, null, "repo-2")));

  expect((dialog.getByLabelText("Model") as HTMLSelectElement).value).toBe("");
});

test("a remembered Default pre-fills Default", () => {
  const { emit, sent } = launch(modelsChanged(listed));
  const dialog = newSession();

  act(() => emit(prefill({ type: "default" })));

  expect((dialog.getByLabelText("Model") as HTMLSelectElement).value).toBe("");
  start(dialog);
  expect(startedModel(sent)).toEqual({ type: "default" });
});

test("a remembered Model no longer offered pre-fills Default with a one-line note", () => {
  const { emit, sent } = launch(modelsChanged(listed));
  const dialog = newSession();

  act(() => emit(prefill({ type: "default" }, "claude-opus-4-1")));

  expect((dialog.getByLabelText("Model") as HTMLSelectElement).value).toBe("");
  expect(dialog.getByRole("note", { name: "Remembered Model" }).textContent).toBe(
    "claude-opus-4-1, your last choice here, is no longer offered: Default instead",
  );
  start(dialog);
  expect(startedModel(sent)).toEqual({ type: "default" });
});

test("picking a Model clears the retired note", () => {
  const { emit } = launch(modelsChanged(listed));
  const dialog = newSession();
  act(() => emit(prefill({ type: "default" }, "claude-opus-4-1")));

  fireEvent.change(dialog.getByLabelText("Model"), { target: { value: "opus" } });

  expect(dialog.queryByRole("note", { name: "Remembered Model" })).toBeNull();
});

test("with no list cached, the remembered id is offered as typed text", () => {
  const { emit, sent } = launch(modelsChanged({ type: "empty" }));
  const dialog = newSession();

  act(() => emit(prefill({ type: "model", id: "claude-opus-4-1" })));

  expect((dialog.getByLabelText("Model") as HTMLInputElement).value).toBe("claude-opus-4-1");
  start(dialog);
  expect(startedModel(sent)).toEqual({ type: "model", id: "claude-opus-4-1" });
});

test("a pre-fill that arrives after the user picked never replaces the pick", () => {
  const { emit, sent } = launch(modelsChanged(listed));
  const dialog = newSession();
  fireEvent.change(dialog.getByLabelText("Model"), { target: { value: "opus" } });

  act(() => emit(prefill(sonnet)));

  start(dialog);
  expect(startedModel(sent)).toEqual({ type: "model", id: "opus" });
});

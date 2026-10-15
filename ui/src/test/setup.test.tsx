import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { App } from "../App";
import type { AgentSetupState } from "../schema/AgentSetupState";
import type { Event } from "../schema/Event";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/fix", branch: "fix/readme", primary: false } },
];

const claude = (state: AgentSetupState): Event => ({
  type: "agentsChecked",
  agents: [{ agent: "claudeCode", name: "Claude Code", state }],
});

const missing = claude({
  type: "missing",
  installHint: "curl -fsSL https://claude.ai/install.sh | bash",
  link: { label: "How to install Claude Code", url: "https://code.claude.com/docs/en/setup" },
});
const tooNew = claude({
  type: "unsupported",
  version: "2.2.0",
  message: "Claude Code 2.2.0 is not yet supported by this build of Switchyard. Check for a Switchyard update.",
  link: { label: "Get the latest Switchyard", url: "https://github.com/hashirventhodi/switchyard/releases" },
});

function launch(...events: Event[]) {
  const fake = fakeCore();
  const openLink = vi.fn();
  render(<App core={fake.core} pickFolder={async () => null} openLink={openLink} />);
  act(() => fake.emit(...repoEvents, ...events));
  return { ...fake, openLink };
}

const openNewSession = () => {
  fireEvent.click(screen.getByRole("button", { name: "New session" }));
  return screen.getByRole("dialog", { name: "New session" });
};

test("the Dashboard shows a setup state listing each missing CLI with its install hint", () => {
  const { sent, openLink } = launch(missing);

  const setup = screen.getByRole("region", { name: "Setup" });
  expect(within(setup).getByText("Claude Code is not installed")).toBeTruthy();
  expect(within(setup).getByText("curl -fsSL https://claude.ai/install.sh | bash")).toBeTruthy();

  fireEvent.click(within(setup).getByRole("button", { name: "How to install Claude Code" }));
  expect(openLink).toHaveBeenCalledWith("https://code.claude.com/docs/en/setup");

  fireEvent.click(within(setup).getByRole("button", { name: "Check again" }));
  expect(sent).toContainEqual({ type: "checkAgents" });
});

test("the Dashboard has no setup state when every agent is ready", () => {
  launch(claude({ type: "ready", version: "2.1.300" }));

  expect(screen.queryByRole("region", { name: "Setup" })).toBeNull();
});

test("an unsupported CLI shows its message and update link on the Dashboard", () => {
  const { openLink } = launch(tooNew);

  const setup = screen.getByRole("region", { name: "Setup" });
  expect(within(setup).getByText(/Claude Code 2.2.0 is not yet supported by this build/)).toBeTruthy();
  fireEvent.click(within(setup).getByRole("button", { name: "Get the latest Switchyard" }));
  expect(openLink).toHaveBeenCalledWith("https://github.com/hashirventhodi/switchyard/releases");
});

test("New session is blocked for an unsupported CLI with the message and an update link", () => {
  const { sent, openLink } = launch(tooNew);

  const dialog = openNewSession();
  fireEvent.change(within(dialog).getByLabelText("Prompt"), { target: { value: "Fix the README" } });

  expect(within(dialog).getByRole("alert").textContent).toContain("Claude Code 2.2.0 is not yet supported by this build");
  expect(within(dialog).getByRole("button", { name: "Start session" })).toHaveProperty("disabled", true);
  fireEvent.click(within(dialog).getByRole("button", { name: "Get the latest Switchyard" }));
  expect(openLink).toHaveBeenCalledWith("https://github.com/hashirventhodi/switchyard/releases");
  expect(sent.filter((c) => c.type === "startSession")).toEqual([]);
});

test("New session is blocked for a missing CLI with its install hint", () => {
  launch(missing);

  const dialog = openNewSession();

  const alert = within(dialog).getByRole("alert");
  expect(alert.textContent).toContain("Claude Code is not installed");
  expect(alert.textContent).toContain("curl -fsSL https://claude.ai/install.sh | bash");
  expect(within(dialog).getByRole("button", { name: "Start session" })).toHaveProperty("disabled", true);
});

test("New session is open once the CLI is ready", () => {
  const { emit } = launch(missing);
  act(() => emit(claude({ type: "ready", version: "2.1.300" })));

  const dialog = openNewSession();
  fireEvent.change(within(dialog).getByLabelText("Prompt"), { target: { value: "Fix the README" } });

  expect(within(dialog).queryByRole("alert")).toBeNull();
  expect(within(dialog).getByRole("button", { name: "Start session" })).toHaveProperty("disabled", false);
});

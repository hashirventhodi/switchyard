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
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/feat", branch: "feat/login", primary: false } },
];
const session: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/fix",
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

function openWorktrees() {
  fireEvent.click(screen.getByRole("button", { name: "Worktrees" }));
}

const card = (branch: string) => screen.getByRole("article", { name: `Worktree ${branch}` });

test("the Worktrees view asks git for fresh facts and shows a card per worktree", () => {
  const { sent, emit } = launch({ type: "sessionStarted", session });

  openWorktrees();
  expect(sent).toContainEqual({ type: "refreshWorktrees" });
  act(() =>
    emit(
      { type: "worktreeFactsChanged", worktreePath: "/wt/acme/fix", facts: { ahead: 2, behind: 0, dirty: 3, conflicts: [] } },
      { type: "worktreeFactsChanged", worktreePath: "/wt/acme/feat", facts: { ahead: 0, behind: 4, dirty: 0, conflicts: [] } },
    ),
  );

  const fix = card("fix/readme");
  expect(fix.className).not.toContain("free");
  expect(fix.textContent).toContain("/wt/acme/fix");
  expect(fix.textContent).toContain("base main");
  expect(fix.textContent).toContain("↑2 ↓0");
  expect(fix.textContent).toContain("dirty 3");
  expect(within(fix).getByTestId("status").textContent).toBe("Running");
  expect(within(fix).getByRole("button", { name: "Open session" })).toBeTruthy();

  const feat = card("feat/login");
  expect(feat.className).toContain("free");
  expect(feat.textContent).toContain("↑0 ↓4");
  expect(feat.textContent).toContain("behind base");
  expect(within(feat).getByRole("button", { name: "Start agent here" })).toBeTruthy();
  expect(within(feat).getByRole("button", { name: "Terminal" })).toBeTruthy();
});

test("Remove asks inline, then removes through the core; a refusal is a toast naming the reason", () => {
  const { sent, emit } = launch();
  openWorktrees();

  fireEvent.click(within(card("feat/login")).getByRole("button", { name: "Remove" }));
  expect(sent).not.toContainEqual(expect.objectContaining({ type: "removeWorktree" }));
  fireEvent.click(within(card("feat/login")).getByRole("button", { name: "Remove worktree" }));
  expect(sent).toContainEqual({ type: "removeWorktree", worktreePath: "/wt/acme/feat" });

  act(() =>
    emit({
      type: "commandRefused",
      command: "removeWorktree",
      reason: "feat/login has 2 uncommitted files; commit or discard them before removing the worktree",
    }),
  );
  expect(screen.getByRole("status").textContent).toContain("feat/login has 2 uncommitted files");
  expect(card("feat/login")).toBeTruthy();

  act(() => emit({ type: "worktreeRemoved", repositoryId: "repo-1", worktreePath: "/wt/acme/feat" }));
  expect(screen.queryByRole("article", { name: "Worktree feat/login" })).toBeNull();
});

test("Esc cancels an inline remove confirm", () => {
  const { sent } = launch();
  openWorktrees();

  fireEvent.click(within(card("feat/login")).getByRole("button", { name: "Remove" }));
  fireEvent.keyDown(window, { key: "Escape" });

  expect(within(card("feat/login")).queryByRole("button", { name: "Remove worktree" })).toBeNull();
  expect(sent).not.toContainEqual(expect.objectContaining({ type: "removeWorktree" }));
});

test("the primary worktree card offers Explore and Quick fix, never a working session or Remove", () => {
  launch();
  openWorktrees();

  const main = card("main");
  expect(within(main).queryByRole("button", { name: "Start agent here" })).toBeNull();
  expect(within(main).queryByRole("button", { name: "Remove" })).toBeNull();
  expect(within(main).getByRole("button", { name: "Explore here" })).toBeTruthy();
  expect(within(main).getByRole("button", { name: "Quick fix" })).toBeTruthy();
});

test("New worktree creates a branch from base in the chosen repository", () => {
  const { sent } = launch();
  openWorktrees();

  fireEvent.click(screen.getByRole("button", { name: "New worktree" }));
  const dialog = screen.getByRole("dialog", { name: "New worktree" });
  expect(dialog.textContent).toContain("from main");
  fireEvent.change(within(dialog).getByLabelText("Branch"), { target: { value: "feat/search" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

  expect(sent).toContainEqual({ type: "createWorktree", repositoryId: "repo-1", branch: "feat/search" });
  expect(screen.queryByRole("dialog", { name: "New worktree" })).toBeNull();
});

test("New session creates a new worktree from base by default, naming the branch from the prompt", () => {
  const { sent, emit } = launch();

  fireEvent.click(screen.getByRole("button", { name: "New session" }));
  const dialog = screen.getByRole("dialog", { name: "New session" });
  expect(within(dialog).getByRole("radio", { name: /Create new from base/ })).toHaveProperty("checked", true);
  fireEvent.change(within(dialog).getByLabelText("Prompt"), { target: { value: "Add cursor pagination to orders" } });
  expect((within(dialog).getByLabelText("Branch") as HTMLInputElement).value).toBe("feat/cursor-pagination-orders");
  fireEvent.click(within(dialog).getByRole("button", { name: "Start session" }));

  expect(sent).toContainEqual({
    type: "startSessionOnNewWorktree",
    repositoryId: "repo-1",
    branch: "feat/cursor-pagination-orders",
    agent: "claudeCode",
    permissionMode: "askFirst",
    prompt: "Add cursor pagination to orders",
  });

  act(() =>
    emit(
      {
        type: "worktreeAdded",
        repositoryId: "repo-1",
        worktree: { path: "/root/acme-1/feat-cursor", branch: "feat/cursor-pagination-orders", primary: false },
      },
      {
        type: "sessionStarted",
        session: { ...session, id: "session-2", worktreePath: "/root/acme-1/feat-cursor", branch: "feat/cursor-pagination-orders" },
      },
    ),
  );
  expect(screen.getByRole("region", { name: "Session feat/cursor-pagination-orders" })).toBeTruthy();
});

test("Quick fix on the primary worktree opens New session on a new fix/ branch", () => {
  const { sent } = launch();
  openWorktrees();

  fireEvent.click(within(card("main")).getByRole("button", { name: "Quick fix" }));
  const dialog = screen.getByRole("dialog", { name: "New session" });
  expect(within(dialog).getByRole("radio", { name: /Create new from base/ })).toHaveProperty("checked", true);
  fireEvent.change(within(dialog).getByLabelText("Prompt"), { target: { value: "Update the copyright year" } });
  expect((within(dialog).getByLabelText("Branch") as HTMLInputElement).value).toBe("fix/copyright-year");
  fireEvent.click(within(dialog).getByRole("button", { name: "Start session" }));

  expect(sent).toContainEqual(
    expect.objectContaining({ type: "startSessionOnNewWorktree", repositoryId: "repo-1", branch: "fix/copyright-year" }),
  );
});

test("a working session on the primary worktree cannot be started from New session", () => {
  launch();

  fireEvent.click(screen.getByRole("button", { name: "New session" }));
  const dialog = screen.getByRole("dialog", { name: "New session" });
  fireEvent.click(within(dialog).getByRole("radio", { name: /Use existing/ }));
  const worktree = within(dialog).getByLabelText("Worktree") as HTMLSelectElement;
  const primary = () => [...worktree.options].find((o) => o.value === "/src/acme")!;

  for (const mode of ["Ask first", "Auto edit", "Full auto"]) {
    const radio = within(dialog).getByRole("radio", { name: mode }) as HTMLInputElement;
    if (!radio.disabled) fireEvent.click(radio);
    expect(primary().disabled, mode).toBe(true);
  }
  fireEvent.click(within(dialog).getByRole("radio", { name: "Explore" }));
  expect(primary().disabled).toBe(false);
});

test("Explore here opens New session on the primary worktree in Explore", () => {
  const { sent } = launch();
  openWorktrees();

  fireEvent.click(within(card("main")).getByRole("button", { name: "Explore here" }));
  const dialog = screen.getByRole("dialog", { name: "New session" });
  expect(within(dialog).getByRole("radio", { name: "Explore" })).toHaveProperty("checked", true);
  fireEvent.change(within(dialog).getByLabelText("Prompt"), { target: { value: "How does auth work?" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Start session" }));

  expect(sent).toContainEqual({
    type: "startSession",
    worktreePath: "/src/acme",
    agent: "claudeCode",
    permissionMode: "explore",
    prompt: "How does auth work?",
  });
});

test("Start agent here opens New session on that worktree", () => {
  launch();
  openWorktrees();

  fireEvent.click(within(card("feat/login")).getByRole("button", { name: "Start agent here" }));

  const dialog = screen.getByRole("dialog", { name: "New session" });
  expect(within(dialog).getByRole("radio", { name: /Use existing/ })).toHaveProperty("checked", true);
  expect((within(dialog).getByLabelText("Worktree") as HTMLSelectElement).value).toBe("/wt/acme/feat");
});

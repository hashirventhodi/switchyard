// Context's Instructions and Memory sections: the files the agent reads, read-only (story 68).
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: null };
const fix: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  status: "running",
  startedAtMs: 0,
};

function openContext() {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() =>
    fake.emit(
      { type: "attached" },
      { type: "repositoryAdded", repository: acme },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/fix", branch: "fix/readme", primary: false } },
      { type: "sessionStarted", session: fix },
    ),
  );
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  fireEvent.click(screen.getByRole("tab", { name: "Context" }));
  return fake;
}

const contextRead: Event = {
  type: "contextRead",
  worktreePath: "/wt/fix",
  files: [
    { name: "CLAUDE.md", path: "/wt/fix/CLAUDE.md", kind: "instructions", text: "Use pnpm.", truncated: false },
    { name: "AGENTS.md", path: "/wt/fix/AGENTS.md", kind: "instructions", text: "Run the tests first.", truncated: true },
    { name: "MEMORY.md", path: "/home/.claude/projects/-wt-fix/memory/MEMORY.md", kind: "memory", text: "- pnpm", truncated: false },
  ],
};

test("Context reads the instruction files and memory, and shows each read-only", () => {
  const { sent, emit } = openContext();
  expect(sent).toContainEqual({ type: "readContext", worktreePath: "/wt/fix" });
  act(() => emit(contextRead));

  const instructions = screen.getByRole("region", { name: "Instructions" });
  expect(within(instructions).getAllByRole("listitem").map((l) => l.querySelector(".k")?.textContent)).toEqual([
    "CLAUDE.md",
    "AGENTS.md",
  ]);
  fireEvent.click(within(instructions).getByRole("button", { name: "Show CLAUDE.md" }));
  expect(within(instructions).getByText("Use pnpm.").tagName).toBe("PRE");
  fireEvent.click(within(instructions).getByRole("button", { name: "Show AGENTS.md" }));
  expect(within(instructions).getByText(/only the start/i)).toBeTruthy();
  expect(within(instructions).queryByRole("textbox")).toBeNull();

  const memory = screen.getByRole("region", { name: "Memory" });
  expect(within(memory).getByText("MEMORY.md")).toBeTruthy();
});

test("with no context files Context says where they would go", () => {
  const { emit } = openContext();
  act(() => emit({ type: "contextRead", worktreePath: "/wt/fix", files: [] }));
  expect(screen.getByRole("region", { name: "Instructions" }).textContent).toContain("No CLAUDE.md or AGENTS.md");
  expect(screen.getByRole("region", { name: "Memory" }).textContent).toContain("No memory");
});

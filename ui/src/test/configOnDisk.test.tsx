// Configuration edited on disk reaches open views without a click (ADR-0008): the core
// pushes contextRead / policyChanged / mcpServersChanged / tasksChanged and the Context tab
// and Tasks view follow while open, with no read of their own.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import type { Task } from "../schema/Task";
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
  model: { type: "default" },
  status: "running",
  startedAtMs: 0,
};

function launch() {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() =>
    fake.emit(
      { type: "attached" },
      { type: "repositoryAdded", repository: acme },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/fix", branch: "fix/readme", primary: false } },
      { type: "sessionStarted", session: fix },
    ),
  );
  return fake;
}

/// Open the session's Context tab, answer its reads, and forget what it sent.
function openContext() {
  const fake = launch();
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  fireEvent.click(screen.getByRole("tab", { name: "Context" }));
  act(() =>
    fake.emit(
      { type: "contextRead", worktreePath: "/wt/fix", files: [instructions("Use npm.")] },
      { type: "policyChanged", repositoryId: "repo-1", policy: policy([]) },
      { type: "mcpServersChanged", repositoryId: "repo-1", error: null, servers: [docs(true, false)] },
    ),
  );
  fake.sent.length = 0;
  return fake;
}

const instructions = (text: string) => ({ name: "CLAUDE.md", path: "/wt/fix/CLAUDE.md", kind: "instructions" as const, text, truncated: false });
const policy = (allow: string[]) => ({
  defaultMode: null,
  ceiling: null,
  grants: [],
  error: null,
  request: allow.length
    ? {
        hash: "abc123",
        allow: allow.map((pattern, i) => ({ id: `repo-allow-${i + 1}`, layer: "repo" as const, effect: "allow" as const, rule: { type: "command" as const, pattern } })),
        sandbox: false,
        defaultMode: null,
        worktreesRoot: null,
        sandboxEvidence: null,
      }
    : null,
});
const docs = (enabled: boolean, changed: boolean) => ({ name: "docs", transport: "http · docs.example.com", enabled, changed });

test("an instruction file edited on disk shows its new content in the open Context tab", () => {
  const { sent, emit } = openContext();

  act(() => emit({ type: "contextRead", worktreePath: "/wt/fix", files: [instructions("Use pnpm.")] }));

  const shown = screen.getByRole("region", { name: "Instructions" });
  fireEvent.click(within(shown).getByRole("button", { name: "Show CLAUDE.md" }));
  expect(within(shown).getByText("Use pnpm.")).toBeTruthy();
  expect(sent).toEqual([]);
});

test("the open Context tab re-reads on a config change signal, and only on that kind", () => {
  const { sent, emit } = openContext();

  act(() => emit({ type: "worktreeChanged", worktreePath: "/wt/fix", kinds: ["files", "git"] }));
  expect(sent).toEqual([]);
  act(() => emit({ type: "worktreeChanged", worktreePath: "/wt/fix", kinds: ["config"] }));

  expect(sent).toEqual([{ type: "readContext", worktreePath: "/wt/fix" }]);
});

test("an allow added to the policy on disk shows as a Policy request in the open Context tab", () => {
  const { sent, emit } = openContext();
  expect(screen.queryByRole("region", { name: "Policy request" })).toBeNull();

  act(() => emit({ type: "policyChanged", repositoryId: "repo-1", policy: policy(["pnpm *"]) }));

  expect(screen.getByRole("region", { name: "Policy request" }).textContent).toContain("pnpm *");
  expect(sent).toEqual([]);
});

test("an MCP server edited on disk shows as changed, not enabled, in the open Context tab", () => {
  const { sent, emit } = openContext();
  const declared = () => screen.getByRole("region", { name: "Declared MCP servers" });
  expect(within(declared()).getByRole("listitem").querySelector(".v")?.textContent).toBe("enabled");

  act(() => emit({ type: "mcpServersChanged", repositoryId: "repo-1", error: null, servers: [docs(false, true)] }));

  expect(within(declared()).getByRole("listitem").querySelector(".v")?.textContent).toBe("changed since you enabled it");
  expect(within(declared()).getByRole("button", { name: "Enable docs" })).toBeTruthy();
  expect(sent).toEqual([]);
});

test("the local tasks file edited by hand shows in the open Tasks view", () => {
  const fake = launch();
  fireEvent.click(screen.getByRole("button", { name: "Tasks" }));
  const task = (title: string): Task => ({
    id: "repo-1/local/L-1",
    repositoryId: "repo-1",
    source: "local",
    key: "L-1",
    title,
    description: "",
    kind: "feat",
    column: "todo",
    checklist: [],
    url: null,
  });
  const board = (title: string): Event => ({ type: "tasksChanged", repositoryId: "repo-1", tasks: [task(title)], houseRules: "", checkedAtMs: null });
  act(() => fake.emit(board("Add login")));
  fake.sent.length = 0;

  act(() => fake.emit(board("Add login with SSO")));

  expect(screen.getByRole("article", { name: "Task L-1" }).textContent).toContain("Add login with SSO");
  expect(fake.sent).toEqual([]);
});

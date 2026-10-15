// MCP servers in Context, driven through a fake core: each server the session's CLI was given,
// with its connection state as the CLI reported it (a failed one reads "not connected").
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { McpServerFact } from "../schema/McpServerFact";
import type { Session } from "../schema/Session";
import type { SessionFacts } from "../schema/SessionFacts";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const fix: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  status: "running",
  startedAtMs: 1_760_000_000_000,
};
const facts = (mcpServers: McpServerFact[]): Event => ({
  type: "sessionFactsChanged",
  sessionId: "session-1",
  facts: {
    model: null,
    cliVersion: "2.1.300",
    cliPermissionMode: "default",
    turns: 0,
    modeWarning: null,
    classifierDenials: 0,
    mcpServers,
  } satisfies SessionFacts,
});

function launch() {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() =>
    fake.emit(
      { type: "attached" },
      { type: "repositoryAdded", repository: acme },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/fix", branch: "fix/readme", primary: false } },
      { type: "sessionStarted", session: fix },
      { type: "transcriptEntry", sessionId: "session-1", index: 0, entry: { type: "user", text: "Fix it" } },
    ),
  );
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  fireEvent.click(screen.getByRole("tab", { name: "Context" }));
  return fake;
}

const section = () => screen.getByRole("region", { name: "MCP servers" });

test("Context lists each MCP server with its connection state; a failed one is not connected", () => {
  const { emit } = launch();
  act(() =>
    emit(
      facts([
        { name: "docs", transport: "http · docs.example.com", state: "connected", error: null },
        { name: "github", transport: "stdio · npx", state: "failed", error: "spawn npx ENOENT" },
        { name: "linear", transport: "http · mcp.linear.app", state: "needsAuth", error: null },
        { name: "slow", transport: "stdio · node", state: "pending", error: null },
      ]),
    ),
  );

  expect(within(section()).getByRole("heading").textContent).toBe("MCP servers1/4");
  const rows = within(section()).getAllByRole("listitem");
  expect(rows.map((r) => r.textContent)).toEqual([
    "docshttp · docs.example.comconnected",
    "githubstdio · npxnot connected",
    "linearhttp · mcp.linear.appneeds sign-in",
    "slowstdio · nodeconnecting…",
  ]);
  expect(within(rows[1]).getByText("not connected").getAttribute("title")).toBe("spawn npx ENOENT");
});

test("a later report updates the state in place", () => {
  const { emit } = launch();
  act(() => emit(facts([{ name: "slow", transport: "stdio · node", state: "pending", error: null }])));
  act(() => emit(facts([{ name: "slow", transport: "stdio · node", state: "connected", error: null }])));
  expect(within(section()).getByRole("listitem").textContent).toBe("slowstdio · nodeconnected");
});

test("with no servers Context says where they come from", () => {
  const { emit } = launch();
  act(() => emit(facts([])));
  expect(section().textContent).toContain("No MCP servers. Add them to the repository's .mcp.json.");
});

const declared = () => screen.getByRole("region", { name: "Declared MCP servers" });

test("declared servers from .mcp.json are not enabled until the user enables each one", () => {
  const { sent, emit } = launch();
  expect(sent).toContainEqual({ type: "readMcpServers", repositoryId: "repo-1" });
  act(() =>
    emit({
      type: "mcpServersChanged",
      repositoryId: "repo-1",
      error: null,
      servers: [
        { name: "docs", transport: "http · docs.example.com", enabled: true, changed: false },
        { name: "github", transport: "stdio · npx", enabled: false, changed: false },
        { name: "search", transport: "http · search.example.com", enabled: false, changed: true },
      ],
    }),
  );
  const rows = within(declared()).getAllByRole("listitem");
  expect(rows.map((r) => r.querySelector(".v")?.textContent)).toEqual([
    "enabled",
    "declared, not enabled",
    "changed since you enabled it",
  ]);
  fireEvent.click(within(declared()).getByRole("button", { name: "Enable github" }));
  expect(sent).toContainEqual({ type: "enableMcpServer", repositoryId: "repo-1", name: "github" });
  expect(within(declared()).getByRole("button", { name: "Enable search" })).toBeTruthy();
  fireEvent.click(within(declared()).getByRole("button", { name: "Disable docs" }));
  expect(sent).toContainEqual({ type: "disableMcpServer", repositoryId: "repo-1", name: "docs" });
  expect(declared().textContent).toContain("next session");
});

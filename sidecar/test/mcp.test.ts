// @vitest-environment node
// MCP servers per spawn: the repository's servers (from the core's config) are the query's
// only MCP servers (strict, so no user-level server leaks in), and each server's connection
// state goes out as reported by system/init, then as the SDK settles servers still pending.
import { expect, test } from "vitest";
import { drive, type QueryFn } from "../src/driver.ts";
import type { ProtocolEvent } from "../src/protocol.ts";
import { ClaudeTranslator } from "../src/translate.ts";

const S = "sess-1";
const init = (mcp_servers: object[]) => ({
  type: "system",
  subtype: "init",
  session_id: S,
  permissionMode: "default",
  claude_code_version: "2.1.300",
  model: "claude-opus-5-5",
  mcp_servers,
});
const say = (text: string) => ({
  type: "assistant",
  parent_tool_use_id: null,
  session_id: S,
  message: { content: [{ type: "text", text }] },
});

const docs = { type: "http", url: "https://docs.example.com/mcp", headers: {} };

/** query() that yields `messages` after the first prompt and answers mcpServerStatus() from `statuses`. */
function fakeQuery(messages: object[], statuses: object[][] = []) {
  const calls = { options: undefined as any, statusCalls: 0 };
  const query: QueryFn = ({ prompt, options }) => {
    calls.options = options;
    async function* run() {
      for await (const _ of prompt as AsyncIterable<unknown>) {
        for (const m of messages) yield m;
        return;
      }
    }
    return Object.assign(run(), {
      mcpServerStatus: async () => statuses[Math.min(calls.statusCalls++, statuses.length - 1)] ?? [],
    }) as any;
  };
  return { query, calls };
}

async function run(config: object, messages: object[], statuses: object[][] = []) {
  const events: ProtocolEvent[] = [];
  const { query, calls } = fakeQuery(messages, statuses);
  const commands = (async function* () {
    yield JSON.stringify({ type: "send_text", text: "hi" });
  })();
  await drive({
    config: { cwd: "/wt/acme/fix", permission_mode: "ask_first", claude_path: null, ...config },
    commands,
    emit: (e) => events.push(e),
    query,
  });
  return { events, calls };
}

test("the repository's servers are the query's only MCP servers", async () => {
  const { calls } = await run({ mcp_servers: { docs } }, [init([])]);
  expect(calls.options.mcpServers).toEqual({ docs });
  expect(calls.options.strictMcpConfig).toBe(true);
});

test("with no servers the query is still strict, so no user-level server loads", async () => {
  const { calls } = await run({}, [init([])]);
  expect(calls.options.mcpServers).toEqual({});
  expect(calls.options.strictMcpConfig).toBe(true);
});

test("init reports each server's state; a failed server says why", () => {
  const t = new ClaudeTranslator();
  const [event] = t.ingest(
    JSON.stringify(
      init([
        { name: "docs", status: "connected" },
        { name: "github", status: "failed", error: "spawn npx ENOENT" },
        { name: "linear", status: "needs-auth" },
      ]),
    ),
  );
  expect(event).toMatchObject({
    type: "session_init",
    mcp_servers: [
      { name: "docs", state: "connected", error: null },
      { name: "github", state: "failed", error: "spawn npx ENOENT" },
      { name: "linear", state: "needs_auth", error: null },
    ],
  });
});

test("servers pending at init are reported again once the SDK settles them", async () => {
  const { events, calls } = await run(
    { mcp_servers: { docs } },
    [init([{ name: "docs", status: "pending" }]), say("working"), say("still working")],
    [[{ name: "docs", status: "pending" }], [{ name: "docs", status: "failed", error: "timed out" }]],
  );
  expect(events.filter((e) => e.type === "session_init" || e.type === "mcp_status")).toEqual([
    expect.objectContaining({ type: "session_init", mcp_servers: [{ name: "docs", state: "pending", error: null }] }),
    { type: "mcp_status", servers: [{ name: "docs", state: "failed", error: "timed out" }] },
  ]);
  // Settled: not asked again.
  expect(calls.statusCalls).toBe(2);
});

test("nothing pending, nothing polled", async () => {
  const { events, calls } = await run({ mcp_servers: { docs } }, [init([{ name: "docs", status: "connected" }]), say("ok")]);
  expect(calls.statusCalls).toBe(0);
  expect(events.some((e) => e.type === "mcp_status")).toBe(false);
});

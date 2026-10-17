// @vitest-environment node
// Model listing in the sidecar: list_models reaches the live query's supportedModels() and
// its answer comes back as protocol events, without holding up the commands behind it.
import { expect, test } from "vitest";
import { drive, type QueryFn } from "../src/driver.ts";
import type { ProtocolEvent } from "../src/protocol.ts";

function fakeQuery(supportedModels: () => Promise<unknown[]>) {
  const calls = { prompts: 0, lists: 0 };
  const query: QueryFn = () => {
    async function* run() {}
    return Object.assign(run(), {
      interrupt: async () => undefined,
      supportedModels: () => {
        calls.lists++;
        return supportedModels();
      },
    }) as any;
  };
  return { query, calls };
}

function commands(...lines: object[]) {
  return (async function* () {
    for (const line of lines) yield JSON.stringify(line);
  })();
}

const prompt = { type: "send_text", text: "Ship it." };
const listModels = { type: "list_models" };

async function run(query: QueryFn, ...lines: object[]) {
  const events: ProtocolEvent[] = [];
  await drive({
    config: { cwd: "/wt", permission_mode: "ask_first", claude_path: null },
    commands: commands(...lines),
    emit: (e) => events.push(e),
    query,
  });
  await new Promise((r) => setTimeout(r, 10));
  return events;
}

const sdkModels = [
  { value: "default", displayName: "Default (recommended)", description: "" },
  { value: "sonnet", resolvedModel: "claude-sonnet-5-5", displayName: "Sonnet", description: "" },
];

test("list_models asks the live query which Models it offers, each with the full id an alias stands for", async () => {
  const { query, calls } = fakeQuery(async () => [...sdkModels, { value: "claude-haiku-4-5", displayName: "Haiku", description: "" }]);
  const events = await run(query, prompt, listModels);
  expect(calls.lists).toBe(1);
  expect(events).toContainEqual({
    type: "models_listed",
    models: [
      { id: "sonnet", display_name: "Sonnet", resolved_model: "claude-sonnet-5-5" },
      { id: "claude-haiku-4-5", display_name: "Haiku", resolved_model: null },
    ],
  });
});

test("a query that cannot list its Models answers cannot_list_models with the reason", async () => {
  const { query } = fakeQuery(async () => {
    throw new Error("Control request timed out: supported_models");
  });
  const events = await run(query, prompt, listModels);
  expect(events).toContainEqual({ type: "cannot_list_models", reason: "Control request timed out: supported_models" });
  expect(events.filter((e) => e.type === "error")).toEqual([]);
});

test("list_models before the first prompt is answered once the query starts", async () => {
  const { query, calls } = fakeQuery(async () => sdkModels);
  const events = await run(query, listModels, prompt);
  expect(calls.lists).toBe(1);
  expect(events.some((e) => e.type === "models_listed")).toBe(true);
});

test("list_models with no prompt at all starts the CLI only to list its Models", async () => {
  const { query, calls } = fakeQuery(async () => sdkModels);
  let prompted = false;
  const listing: QueryFn = (params) => {
    void (async () => {
      for await (const _ of params.prompt) prompted = true;
    })();
    return query(params);
  };
  const events = await run(listing, listModels);
  expect(calls.lists).toBe(1);
  expect(events.some((e) => e.type === "models_listed")).toBe(true);
  expect(prompted).toBe(false);
});

test("a slow list does not hold up the commands behind it", async () => {
  const { query } = fakeQuery(() => new Promise(() => {}));
  const started: unknown[] = [];
  const counting: QueryFn = (params) => {
    started.push(params);
    return query(params);
  };
  await run(counting, listModels, prompt);
  expect(started).toHaveLength(1);
});

/** Runs a list-only sidecar (the host started it only to list Models), recording what query() got. */
async function runListOnly(supportedModels: () => Promise<unknown[]>, ...lines: object[]) {
  const { query, calls } = fakeQuery(supportedModels);
  const started: { options: any; prompts: unknown[] }[] = [];
  const recording: QueryFn = (params) => {
    const seen = { options: params.options, prompts: [] as unknown[] };
    started.push(seen);
    void (async () => {
      for await (const message of params.prompt) seen.prompts.push(message);
    })();
    return query(params);
  };
  const events: ProtocolEvent[] = [];
  await drive({
    config: {
      cwd: "/tmp/switchyard-models-x",
      permission_mode: "explore",
      claude_path: null,
      allowed_tools: ["Bash(git status)"],
      mcp_servers: { github: { command: "npx" } },
      list_only: true,
    },
    commands: commands(...lines),
    emit: (e) => events.push(e),
    query: recording,
  });
  await new Promise((r) => setTimeout(r, 10));
  return { events, started, calls };
}

test("a list-only sidecar starts the CLI with every tool disallowed, in the most locked-down mode", async () => {
  const { events, started } = await runListOnly(async () => sdkModels, listModels);
  expect(started).toHaveLength(1);
  const options = started[0].options;
  expect(options.tools).toEqual([]);
  expect(options.permissionMode).toBe("dontAsk");
  expect(options.allowedTools).toEqual([]);
  expect(options.mcpServers).toEqual({});
  expect(options.strictMcpConfig).toBe(true);
  expect(options.persistSession).toBe(false);
  expect(options.model).toBeUndefined();
  expect(started[0].prompts).toEqual([]);
  expect(events.some((e) => e.type === "models_listed")).toBe(true);
});

test("a list-only sidecar denies any tool use without asking the host", async () => {
  const { events, started } = await runListOnly(async () => sdkModels, listModels);
  const verdict = await started[0].options.canUseTool("Bash", { command: "rm -rf ~" }, {
    signal: new AbortController().signal,
    requestId: "r1",
    toolUseID: "t1",
  });
  expect(verdict.behavior).toBe("deny");
  expect(events.filter((e) => e.type === "permission_request")).toEqual([]);
});

test("a list-only sidecar refuses every command but list_models", async () => {
  const { events, started, calls } = await runListOnly(
    async () => sdkModels,
    prompt,
    { type: "resume", external_session_id: "cli-1", model: "opus" },
    { type: "set_model", model: "opus" },
    { type: "set_permission_mode", permission_mode: "full_auto" },
    listModels,
  );
  expect(started).toHaveLength(1);
  expect(started[0].prompts).toEqual([]);
  expect(started[0].options.resume).toBeUndefined();
  expect(started[0].options.model).toBeUndefined();
  expect(started[0].options.permissionMode).toBe("dontAsk");
  expect(calls.lists).toBe(1);
  expect(events.filter((e) => e.type === "error")).toHaveLength(4);
  expect(events.some((e) => e.type === "models_listed")).toBe(true);
});

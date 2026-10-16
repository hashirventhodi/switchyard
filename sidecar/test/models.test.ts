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

test("list_models asks the live query which Models it offers", async () => {
  const { query, calls } = fakeQuery(async () => sdkModels);
  const events = await run(query, prompt, listModels);
  expect(calls.lists).toBe(1);
  expect(events).toContainEqual({ type: "models_listed", models: [{ id: "sonnet", display_name: "Sonnet" }] });
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

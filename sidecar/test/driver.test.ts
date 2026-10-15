// @vitest-environment node
// The live driver: protocol commands in on stdin, SDK query() in the middle, protocol events
// out. query() is faked: it records its options and the user messages it is fed, and plays
// scripted SDK messages after each one. No SDK, no CLI, no model.
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "vitest";
import { drive, type QueryFn } from "../src/driver.ts";
import type { ProtocolEvent } from "../src/protocol.ts";

const S = "sess-1";
const init = {
  type: "system",
  subtype: "init",
  session_id: S,
  permissionMode: "default",
  claude_code_version: "2.1.300",
  model: "claude-opus-5-5",
};
const say = (text: string) => ({
  type: "assistant",
  parent_tool_use_id: null,
  session_id: S,
  message: { content: [{ type: "text", text }] },
});
const result = (text: string) => ({
  type: "result",
  subtype: "success",
  is_error: false,
  result: text,
  total_cost_usd: 0.01,
  modelUsage: { m: { inputTokens: 1, outputTokens: 2, cacheReadInputTokens: 3 } },
  session_id: S,
});

/** A channel of lines the test pushes into, standing in for stdin. */
function lines() {
  const queue: string[] = [];
  let wake: (() => void) | null = null;
  let closed = false;
  return {
    push(value: unknown) {
      queue.push(JSON.stringify(value));
      wake?.();
    },
    close() {
      closed = true;
      wake?.();
    },
    async *[Symbol.asyncIterator]() {
      for (;;) {
        if (queue.length) yield queue.shift()!;
        else if (closed) return;
        else await new Promise<void>((r) => (wake = r));
      }
    },
  };
}

/** query() that answers every user message with `reply(text)` messages, then a result. */
function fakeQuery(reply: (text: string) => object[], opts: { initFirst?: boolean; throwAfterFirst?: boolean } = {}) {
  const calls = { options: undefined as any, prompts: [] as string[], interrupts: 0 };
  const query: QueryFn = ({ prompt, options }) => {
    calls.options = options;
    async function* run() {
      yield init;
      for await (const message of prompt as AsyncIterable<any>) {
        const text = message.message.content as string;
        calls.prompts.push(text);
        if (opts.throwAfterFirst && calls.prompts.length > 1) throw new Error("Claude Code process exited with code 1");
        for (const m of reply(text)) yield m;
      }
    }
    const it = run();
    return Object.assign(it, {
      interrupt: async () => {
        calls.interrupts++;
        return undefined;
      },
    }) as any;
  };
  return { query, calls };
}

async function until(events: ProtocolEvent[], type: ProtocolEvent["type"]) {
  for (let i = 0; i < 200 && !events.some((e) => e.type === type); i++) await new Promise((r) => setTimeout(r, 5));
}

test("runs the query in the worktree with the CLI's mode and streams translated events", async () => {
  const stdin = lines();
  const events: ProtocolEvent[] = [];
  const { query, calls } = fakeQuery((text) => [say(`echo ${text}`), result(`echo ${text}`)]);
  const done = drive({
    config: { cwd: "/wt/acme/fix", permission_mode: "ask_first", claude_path: "/usr/local/bin/claude" },
    commands: stdin,
    emit: (e) => events.push(e),
    query,
  });

  stdin.push({ type: "send_text", text: "hi" });
  await until(events, "turn_complete");
  stdin.close();
  await done;

  expect(calls.options).toMatchObject({
    cwd: "/wt/acme/fix",
    permissionMode: "default",
    pathToClaudeCodeExecutable: "/usr/local/bin/claude",
  });
  expect(calls.prompts).toEqual(["hi"]);
  expect(events.map((e) => e.type)).toEqual([
    "session_init",
    "agent_text",
    "token_usage",
    "turn_complete",
    "done",
  ]);
  expect(events[1]).toEqual({ type: "agent_text", text: "echo hi" });
  expect(events[4]).toEqual({ type: "done", summary: "echo hi" });
});

test("interrupt calls query.interrupt()", async () => {
  const stdin = lines();
  const { query, calls } = fakeQuery(() => []);
  const done = drive({
    config: { cwd: "/wt", permission_mode: "ask_first", claude_path: null },
    commands: stdin,
    emit: () => {},
    query,
  });

  stdin.push({ type: "interrupt" });
  stdin.close();
  await done;

  expect(calls.interrupts).toBe(1);
});

test("a query that throws ends with a fatal error", async () => {
  const stdin = lines();
  const events: ProtocolEvent[] = [];
  const { query } = fakeQuery((t) => [result(t)], { throwAfterFirst: true });
  const done = drive({
    config: { cwd: "/wt", permission_mode: "ask_first", claude_path: null },
    commands: stdin,
    emit: (e) => events.push(e),
    query,
  });

  stdin.push({ type: "send_text", text: "one" });
  stdin.push({ type: "send_text", text: "two" });
  await done;

  expect(events.at(-1)).toEqual({ type: "error", message: "Claude Code process exited with code 1", fatal: true });
});

test("a command the adapter cannot carry out is reported, not fatal", async () => {
  const stdin = lines();
  const events: ProtocolEvent[] = [];
  const { query } = fakeQuery(() => []);
  const done = drive({
    config: { cwd: "/wt", permission_mode: "ask_first", claude_path: null },
    commands: stdin,
    emit: (e) => events.push(e),
    query,
  });

  stdin.push({ type: "resume", external_session_id: "x" });
  stdin.close();
  await done;

  expect(events).toContainEqual({ type: "error", message: "resume is not supported yet", fatal: false });
});

test("with a record directory it writes a replayable fixture of the run", async () => {
  const dir = mkdtempSync(join(tmpdir(), "switchyard-record-"));
  const stdin = lines();
  const { query } = fakeQuery((text) => [say(text), result(text)]);
  const done = drive({
    config: { cwd: "/wt", permission_mode: "ask_first", claude_path: null },
    commands: stdin,
    emit: () => {},
    query,
    recordDir: dir,
  });

  stdin.push({ type: "send_text", text: "hi" });
  await new Promise((r) => setTimeout(r, 20));
  stdin.close();
  await done;

  const input = readFileSync(join(dir, "input.jsonl"), "utf8").trim().split("\n").map((l) => JSON.parse(l));
  expect(input.map((m) => m.type)).toEqual(["system", "assistant", "result", "switchyard"]);
  const commands = readFileSync(join(dir, "commands.jsonl"), "utf8").trim().split("\n").map((l) => JSON.parse(l));
  // The prompt may be taken before or after init is read; either is a faithful recording.
  expect([0, 1]).toContain(commands[0].after_input);
  expect(commands).toEqual([
    {
      after_input: commands[0].after_input,
      command: { type: "send_text", text: "hi" },
      expect_responses: [
        { op: "push_user_message", message: { type: "user", message: { role: "user", content: "hi" }, parent_tool_use_id: null } },
      ],
    },
  ]);
  const expected = readFileSync(join(dir, "expected-events.jsonl"), "utf8").trim().split("\n").map((l) => JSON.parse(l));
  expect(expected.map((e) => e.type)).toEqual(["session_init", "agent_text", "token_usage", "turn_complete", "done"]);
  expect(JSON.parse(readFileSync(join(dir, "meta.json"), "utf8"))).toMatchObject({
    adapter: "claude-code",
    cli: { name: "claude-code", version: "2.1.300" },
    protocol: "0.1.0",
  });
});

/** query() whose turn asks canUseTool before running `pnpm test`, then reports what it was told. */
function askingQuery() {
  const calls = { options: undefined as any, results: [] as unknown[], controller: new AbortController() };
  const query: QueryFn = ({ prompt, options }) => {
    calls.options = options;
    async function* run() {
      yield init;
      for await (const _ of prompt as AsyncIterable<any>) {
        const result = await (options as any).canUseTool(
          "Bash",
          { command: "pnpm test" },
          { signal: calls.controller.signal, toolUseID: "toolu_1", requestId: "req-1", title: "Claude wants to run pnpm test" },
        );
        calls.results.push(result);
        yield result.behavior === "allow" ? say("ran it") : say("skipped it");
      }
    }
    return Object.assign(run(), { interrupt: async () => undefined }) as any;
  };
  return { query, calls };
}

test("prompts reach the host through canUseTool and wait for the verdict", async () => {
  const stdin = lines();
  const events: ProtocolEvent[] = [];
  const { query, calls } = askingQuery();
  const done = drive({
    config: { cwd: "/wt", permission_mode: "ask_first", claude_path: null },
    commands: stdin,
    emit: (e) => events.push(e),
    query,
  });

  stdin.push({ type: "send_text", text: "test it" });
  await until(events, "permission_request");
  expect(calls.options.permissionPrompts).toBe("host");
  expect(events.at(-1)).toEqual({
    type: "permission_request",
    request_id: "req-1",
    request: { kind: "command", command: "pnpm test", cwd: null },
    reason: null,
  });
  expect(calls.results).toEqual([]);

  stdin.push({ type: "answer_request", request_id: "req-1", verdict: { decision: "allow_once" } });
  await until(events, "agent_text");
  stdin.close();
  await done;

  expect(calls.results).toEqual([{ behavior: "allow", updatedInput: { command: "pnpm test" } }]);
  expect(events.at(-2)).toEqual({ type: "agent_text", text: "ran it" });
});

test("an answer to a request the CLI has abandoned is ignored", async () => {
  const stdin = lines();
  const events: ProtocolEvent[] = [];
  const { query, calls } = askingQuery();
  const done = drive({
    config: { cwd: "/wt", permission_mode: "ask_first", claude_path: null },
    commands: stdin,
    emit: (e) => events.push(e),
    query,
  });

  stdin.push({ type: "send_text", text: "test it" });
  await until(events, "permission_request");
  calls.controller.abort(); // the turn was interrupted
  stdin.push({ type: "answer_request", request_id: "req-1", verdict: { decision: "deny", message: null } });
  await new Promise((r) => setTimeout(r, 20));
  stdin.close();

  expect(calls.results).toEqual([]);
  expect(events.filter((e) => e.type === "error")).toEqual([]);
  void done;
});

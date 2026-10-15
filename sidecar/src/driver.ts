// Connects the translator to a live Agent SDK query(): protocol commands (one JSON line each)
// become driver ops on the query, and every SDK message becomes protocol events. query() is
// injected so tests can drive this without the SDK, the CLI or a model.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Options, Query, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { PermissionMode, ProtocolEvent } from "./protocol.ts";
import { CLI_MODE, ClaudeTranslator, type PermissionResult } from "./translate.ts";

export interface SidecarConfig {
  /** The worktree. */
  cwd: string;
  permission_mode: PermissionMode;
  /** The `claude` executable; null lets the SDK use its default. */
  claude_path: string | null;
}

export type QueryFn = (params: { prompt: AsyncIterable<SDKUserMessage>; options: Options }) => Query;

export interface DriveParams {
  config: SidecarConfig;
  /** Protocol commands, one JSON object per line (stdin). Ending it ends the session. */
  commands: AsyncIterable<string>;
  emit: (event: ProtocolEvent) => void;
  query: QueryFn;
  /** When set, the run is also written there as a conformance fixture. */
  recordDir?: string;
}

export const CLIENT_APP = "switchyard/0.1.0";

export async function drive({ config, commands, emit, query, recordDir }: DriveParams): Promise<void> {
  const translator = new ClaudeTranslator();
  const recorder = recordDir ? new Recorder(recordDir) : null;
  const ingest = (raw: string) => {
    recorder?.input(raw);
    for (const event of translator.ingest(raw)) {
      recorder?.event(event);
      emit(event);
    }
  };

  // canUseTool calls waiting on the host's verdict, by request id. They may wait forever:
  // the request sits in the user's queue until answered or the turn is interrupted.
  const waiting = new Map<string, (result: PermissionResult) => void>();
  const canUseTool = (
    toolName: string,
    input: Record<string, unknown>,
    o: { signal: AbortSignal; requestId: string; toolUseID: string; title?: string; decisionReason?: string; blockedPath?: string },
  ) =>
    new Promise<PermissionResult>((resolve) => {
      waiting.set(o.requestId, resolve);
      o.signal.addEventListener("abort", () => {
        if (waiting.delete(o.requestId))
          ingest(JSON.stringify({ type: "switchyard", subtype: "can_use_tool_aborted", request_id: o.requestId }));
      });
      ingest(
        JSON.stringify({
          type: "switchyard",
          subtype: "can_use_tool",
          request_id: o.requestId,
          tool_name: toolName,
          input,
          tool_use_id: o.toolUseID,
          title: o.title ?? null,
          decision_reason: o.decisionReason ?? null,
          blocked_path: o.blockedPath ?? null,
        }),
      );
    });

  const prompts = new Pushable<SDKUserMessage>();
  // The query starts with the first command: a resume (the CLI reloads that conversation)
  // or the first prompt. A session that never gets a command never starts the CLI.
  let q: Query | null = null;
  let started: (q: Query | null) => void = () => {};
  const ready = new Promise<Query | null>((resolve) => (started = resolve));
  const start = (resume?: string) => {
    q = query({
      prompt: prompts,
      options: {
        cwd: config.cwd,
        // Always passed: when omitted the CLI picks its own default mode.
        permissionMode: CLI_MODE[config.permission_mode] as Options["permissionMode"],
        pathToClaudeCodeExecutable: config.claude_path ?? undefined,
        // Everything that would prompt comes to canUseTool and from there to the user's queue.
        permissionPrompts: "host",
        canUseTool,
        resume,
        env: { ...process.env, CLAUDE_AGENT_SDK_CLIENT_APP: CLIENT_APP },
      } as Options,
    });
    started(q);
    return q;
  };

  const commandLoop = (async () => {
    for await (const line of commands) {
      if (!line.trim()) continue;
      let command;
      let ops;
      try {
        command = JSON.parse(line);
        ops = translator.command(command);
      } catch (e) {
        emit({ type: "error", message: (e as Error).message, fatal: false });
        continue;
      }
      recorder?.command(command, ops);
      for (const op of ops) {
        if (op.op === "resume") {
          if (q) emit({ type: "error", message: "resume must come before any prompt", fatal: false });
          else start(op.session_id);
        } else if (op.op === "push_user_message") {
          q ?? start();
          prompts.push(op.message as SDKUserMessage);
        } else if (op.op === "interrupt") await q?.interrupt();
        else if (op.op === "resolve_permission") {
          waiting.get(op.request_id)?.(op.result);
          waiting.delete(op.request_id);
        }
      }
    }
    prompts.end();
    started(null);
  })();
  commandLoop.catch((e) => emit({ type: "error", message: `command failed: ${(e as Error).message}`, fatal: false }));

  const running = await ready;
  if (!running) return;
  try {
    for await (const message of running) ingest(JSON.stringify(message));
    ingest(JSON.stringify({ type: "switchyard", subtype: "query_end" }));
  } catch (e) {
    ingest(JSON.stringify({ type: "switchyard", subtype: "query_error", message: (e as Error)?.message ?? String(e) }));
  }
  // The command loop may still be waiting on stdin; the session is over either way.
  prompts.end();
}

/** An async iterable the driver pushes user messages into (the SDK's streaming input). */
class Pushable<T> implements AsyncIterable<T> {
  private queue: T[] = [];
  private wake: (() => void) | null = null;
  private ended = false;

  push(value: T) {
    this.queue.push(value);
    this.wake?.();
  }

  end() {
    this.ended = true;
    this.wake?.();
  }

  async *[Symbol.asyncIterator](): AsyncIterator<T> {
    for (;;) {
      if (this.queue.length) yield this.queue.shift()!;
      else if (this.ended) return;
      else await new Promise<void>((resolve) => (this.wake = resolve));
    }
  }
}

/** Writes a run as a conformance fixture directory (see crates/adapter-protocol/README.md). */
class Recorder {
  private inputs = 0;
  private dir: string;

  constructor(dir: string) {
    this.dir = dir;
    mkdirSync(dir, { recursive: true });
    for (const f of ["input.jsonl", "expected-events.jsonl", "commands.jsonl"]) writeFileSync(join(dir, f), "");
  }

  input(raw: string) {
    this.inputs++;
    appendFileSync(join(this.dir, "input.jsonl"), raw + "\n");
  }

  event(event: ProtocolEvent) {
    appendFileSync(join(this.dir, "expected-events.jsonl"), JSON.stringify(event) + "\n");
    if (event.type === "session_init") {
      const meta = {
        adapter: "claude-code",
        cli: event.cli,
        protocol: "0.1.0",
        description: "Recorded with SWITCHYARD_RECORD_DIR. Review expected-events.jsonl before committing.",
      };
      writeFileSync(join(this.dir, "meta.json"), JSON.stringify(meta, null, 2) + "\n");
    }
  }

  command(command: unknown, ops: unknown[]) {
    const line = { after_input: this.inputs, command, expect_responses: ops };
    appendFileSync(join(this.dir, "commands.jsonl"), JSON.stringify(line) + "\n");
  }
}

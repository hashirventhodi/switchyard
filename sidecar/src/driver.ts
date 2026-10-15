// Connects the translator to a live Agent SDK query(): protocol commands (one JSON line each)
// become driver ops on the query, and every SDK message becomes protocol events. query() is
// injected so tests can drive this without the SDK, the CLI or a model.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Options, Query, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { PermissionMode, ProtocolEvent } from "./protocol.ts";
import { classes } from "./dangerous.ts";
import { CLI_MODE, ClaudeTranslator, type PermissionResult } from "./translate.ts";

export interface SidecarConfig {
  /** The worktree. */
  cwd: string;
  permission_mode: PermissionMode;
  /** The `claude` executable; null lets the SDK use its default. */
  claude_path: string | null;
  /** Switchyard's resolved Grants as CLI permission rules (`--allowedTools`). */
  allowed_tools?: string[];
  /** Switchyard's resolved denials as CLI permission rules (`--disallowedTools`). */
  disallowed_tools?: string[];
  /** The repository's policy declares a sandbox: the CLI's bypass mode may be used. */
  allow_bypass?: boolean;
}

/** Modes in which dangerous commands always reach the host (everything below Full auto). */
const GUARDED: PermissionMode[] = ["explore", "ask_first", "auto_edit"];

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

  // The mode the CLI is in now (the composer can switch it), for the dangerous-command hook.
  let mode = config.permission_mode;
  // Below Full auto, network, delete, migration and push commands are asked of the host
  // even when the CLI's mode (acceptEdits approves rm) or an allow rule would approve them.
  const guardDangerous = async (input: { tool_name?: string; tool_input?: { command?: unknown } }) => {
    const command = input.tool_input?.command;
    if (!GUARDED.includes(mode) || typeof command !== "string") return {};
    const found = classes(command);
    if (!found.length) return {};
    return {
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "ask",
        permissionDecisionReason: `Switchyard always asks before ${found.join(", ")} commands in this mode`,
      },
    };
  };

  const prompts = new Pushable<SDKUserMessage>();
  // The query starts with the first command: a resume (the CLI reloads that conversation)
  // or the first prompt. A session that never gets a command never starts the CLI.
  let q = null as Query | null;
  // The CLI-facing mode to start in; a switch before the first prompt changes it.
  let cliMode: string = CLI_MODE[config.permission_mode];
  let started: (q: Query | null) => void = () => {};
  const ready = new Promise<Query | null>((resolve) => (started = resolve));
  const start = (resume?: string) => {
    q = query({
      prompt: prompts,
      options: {
        cwd: config.cwd,
        // Always passed: when omitted the CLI picks its own default mode.
        permissionMode: cliMode as Options["permissionMode"],
        // Bypass-everything is only ever made available where the policy declares a sandbox.
        ...(config.allow_bypass ? { allowDangerouslySkipPermissions: true } : {}),
        pathToClaudeCodeExecutable: config.claude_path ?? undefined,
        // Grants are Switchyard's data, passed per spawn as flags; never written into the
        // CLI's settings files (ADR-0004).
        allowedTools: config.allowed_tools ?? [],
        disallowedTools: config.disallowed_tools ?? [],
        // Everything that would prompt comes to canUseTool and from there to the user's queue.
        permissionPrompts: "host",
        canUseTool,
        hooks: { PreToolUse: [{ matcher: "Bash", hooks: [guardDangerous] }] },
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
        else if (op.op === "set_permission_mode") {
          if (q) await q.setPermissionMode(op.mode as Parameters<Query["setPermissionMode"]>[0]);
          else cliMode = op.mode;
          mode = op.permission_mode;
        }
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

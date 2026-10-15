// The Claude Code adapter proper: a pure translator between the Agent SDK's message
// stream (one SDKMessage per raw line) and the adapter protocol. It never touches the SDK
// or a process, so recorded fixtures drive it deterministically (see replay.ts); main.ts
// connects it to a live query().
//
// Raw input lines are SDK messages as the SDK yields them, plus two lines the sidecar
// writes itself:
//   {"type":"switchyard","subtype":"query_end"}                 the SDK stream ended
//   {"type":"switchyard","subtype":"query_error","message":"…"} query() threw
//
// Raw responses are "driver ops" the sidecar performs on the live query:
//   {"op":"push_user_message","message":SDKUserMessage}  push into the streaming input
//   {"op":"interrupt"}                                   await query.interrupt()

import { PROTOCOL_RANGE, type PermissionMode, type ProtocolCommand, type ProtocolEvent } from "./protocol.ts";

export const MANIFEST = { name: "claude-code", implements: PROTOCOL_RANGE };

/** Switchyard's permission mode to the CLI's (`--permission-mode`). */
export const CLI_MODE: Record<PermissionMode, string> = {
  explore: "plan",
  ask_first: "default",
  auto_edit: "acceptEdits",
  full_auto: "auto",
  bypass: "bypassPermissions",
};

/** The CLI's mode, as reported in system/init, back to Switchyard's. */
function switchyardMode(cliMode: string): PermissionMode {
  switch (cliMode) {
    case "plan":
      return "explore";
    case "acceptEdits":
      return "auto_edit";
    case "auto":
      return "full_auto";
    case "bypassPermissions":
      return "bypass";
    default:
      // "default" / "manual"; "dontAsk" (deny anything not pre-approved) is closest to Ask first.
      return "ask_first";
  }
}

export type DriverOp =
  | { op: "push_user_message"; message: { type: "user"; message: { role: "user"; content: string }; parent_tool_use_id: null } }
  | { op: "interrupt" };

export class AdapterError extends Error {}

// Loose views of the SDK messages we read; everything else is ignored.
type Json = Record<string, any>;

export class ClaudeTranslator {
  private initialisedSession: string | null = null;
  private lastResultText: string | null = null;

  ingest(raw: string): ProtocolEvent[] {
    let msg: Json;
    try {
      msg = JSON.parse(raw);
    } catch (e) {
      throw new AdapterError(`not JSON: ${(e as Error).message}`);
    }
    switch (msg.type) {
      case "system":
        return this.system(msg);
      case "assistant":
        return this.assistant(msg);
      case "user":
        return this.user(msg);
      case "result":
        return this.result(msg);
      case "switchyard":
        return this.sidecar(msg);
      default:
        return [];
    }
  }

  command(command: ProtocolCommand): DriverOp[] {
    switch (command.type) {
      case "send_text":
        return [
          {
            op: "push_user_message",
            message: { type: "user", message: { role: "user", content: command.text }, parent_tool_use_id: null },
          },
        ];
      case "interrupt":
        return [{ op: "interrupt" }];
      case "answer_request":
        throw new AdapterError("answering requests is not supported yet (permission relay)");
      case "resume":
        throw new AdapterError("resume is not supported yet");
    }
  }

  private system(msg: Json): ProtocolEvent[] {
    if (msg.subtype === "init") {
      if (this.initialisedSession === msg.session_id) return [];
      this.initialisedSession = msg.session_id;
      return [
        {
          type: "session_init",
          external_session_id: msg.session_id,
          permission_mode: switchyardMode(msg.permissionMode),
          cli_permission_mode: msg.permissionMode,
          cli: { name: "claude-code", version: msg.claude_code_version },
          model: msg.model ?? null,
        },
      ];
    }
    if (msg.subtype === "permission_denied" && msg.decision_reason_type === "classifier") {
      return [
        { type: "classifier_denial", tool: msg.tool_name, detail: msg.message ?? "", reason: msg.decision_reason ?? null },
      ];
    }
    return [];
  }

  private assistant(msg: Json): ProtocolEvent[] {
    // Subagent traffic (parent_tool_use_id set) is summarised by its parent Task tool call.
    if (msg.parent_tool_use_id) return [];
    const events: ProtocolEvent[] = [];
    for (const block of msg.message?.content ?? []) {
      if (block.type === "text" && block.text) events.push({ type: "agent_text", text: block.text });
      else if (block.type === "tool_use")
        events.push({ type: "tool_call_started", call_id: block.id, tool: block.name, input: block.input ?? {} });
    }
    return events;
  }

  private user(msg: Json): ProtocolEvent[] {
    if (msg.parent_tool_use_id) return [];
    const content = msg.message?.content;
    if (!Array.isArray(content)) return [];
    return content
      .filter((block: Json) => block.type === "tool_result")
      .map((block: Json) => ({
        type: "tool_call_finished" as const,
        call_id: block.tool_use_id,
        ok: !block.is_error,
        output: resultText(block.content),
      }));
  }

  private result(msg: Json): ProtocolEvent[] {
    const usage = Object.values((msg.modelUsage ?? {}) as Record<string, Json>).reduce(
      (sum, m) => ({
        input: sum.input + (m.inputTokens ?? 0),
        output: sum.output + (m.outputTokens ?? 0),
        cacheRead: sum.cacheRead + (m.cacheReadInputTokens ?? 0),
      }),
      { input: 0, output: 0, cacheRead: 0 },
    );
    const events: ProtocolEvent[] = [
      {
        type: "token_usage",
        input_tokens: usage.input,
        output_tokens: usage.output,
        cache_read_tokens: usage.cacheRead,
        cost_usd: typeof msg.total_cost_usd === "number" ? msg.total_cost_usd : null,
      },
    ];
    const interrupted = typeof msg.terminal_reason === "string" && msg.terminal_reason.startsWith("aborted");
    if (msg.subtype === "success" && !msg.is_error) {
      this.lastResultText = msg.result ?? null;
    } else if (!interrupted) {
      const message =
        (Array.isArray(msg.errors) && msg.errors.length ? msg.errors.join("\n") : msg.result) || `turn failed: ${msg.subtype}`;
      events.push({ type: "error", message, fatal: false });
    }
    events.push({ type: "turn_complete" });
    return events;
  }

  private sidecar(msg: Json): ProtocolEvent[] {
    if (msg.subtype === "query_end") return [{ type: "done", summary: this.lastResultText }];
    if (msg.subtype === "query_error") return [{ type: "error", message: String(msg.message), fatal: true }];
    return [];
  }
}

function resultText(content: unknown): string | null {
  if (typeof content === "string") return content;
  if (Array.isArray(content))
    return content
      .filter((c: Json) => c.type === "text")
      .map((c: Json) => c.text)
      .join("\n");
  return null;
}

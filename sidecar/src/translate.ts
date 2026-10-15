// The Claude Code adapter proper: a pure translator between the Agent SDK's message
// stream (one SDKMessage per raw line) and the adapter protocol. It never touches the SDK
// or a process, so recorded fixtures drive it deterministically (see replay.ts); main.ts
// connects it to a live query().
//
// Raw input lines are SDK messages as the SDK yields them, plus lines the sidecar writes
// itself:
//   {"type":"switchyard","subtype":"query_end"}                 the SDK stream ended
//   {"type":"switchyard","subtype":"query_error","message":"…"} query() threw
//   {"type":"switchyard","subtype":"can_use_tool","request_id","tool_name","input",
//    "tool_use_id","title","decision_reason","blocked_path"}    canUseTool was called
//   {"type":"switchyard","subtype":"can_use_tool_aborted","request_id"}
//                                                               its signal aborted (interrupt)
//   {"type":"switchyard","subtype":"mcp_status","servers":[McpServerStatus]}
//                                                               query.mcpServerStatus() result
//                                                               (asked while a server is pending)
//
// Raw responses are "driver ops" the sidecar performs on the live query:
//   {"op":"push_user_message","message":SDKUserMessage}  push into the streaming input
//   {"op":"interrupt"}                                   await query.interrupt()
//   {"op":"resolve_permission","request_id","result":PermissionResult}
//                                                        settle that canUseTool call
//   {"op":"resume","session_id"}                         start the query on that CLI session
//                                                        (options.resume); first op only
//   {"op":"set_permission_mode","mode":"<CLI mode>","permission_mode":"<protocol mode>"}
//                                                        await query.setPermissionMode(mode)

import {
  PROTOCOL_RANGE,
  type McpServerState,
  type McpServerStatus,
  type PermissionMode,
  type ProtocolCommand,
  type ProtocolEvent,
  type RequestDetail,
  type Verdict,
} from "./protocol.ts";

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
  | { op: "interrupt" }
  | { op: "resolve_permission"; request_id: string; result: PermissionResult }
  | { op: "resume"; session_id: string }
  | { op: "set_permission_mode"; mode: string; permission_mode: PermissionMode };

/** The Agent SDK's answer to canUseTool. Never carries `updatedPermissions`: grants are
 * Switchyard data and are never written into the CLI's settings (ADR-0004). */
export type PermissionResult =
  | { behavior: "allow"; updatedInput: Record<string, unknown> }
  | { behavior: "deny"; message: string };

/** What the agent is told when the user skips a request without saying why. */
export const DECLINED = "The user declined this request.";

/** The SDK's built-in question tool: a question request, never a tool call. */
const QUESTION_TOOL = "AskUserQuestion";
/** Plan mode's "done planning" tool. Explore never leaves plan mode, so it is asked as a
 * question whose answer goes back as feedback while the agent keeps exploring. */
const PLAN_TOOL = "ExitPlanMode";

export class AdapterError extends Error {}

// Loose views of the SDK messages we read; everything else is ignored.
type Json = Record<string, any>;

export class ClaudeTranslator {
  private initialisedSession: string | null = null;
  private lastResultText: string | null = null;
  /** canUseTool calls awaiting a verdict, by request id. */
  private pending = new Map<string, { tool: string; input: Json }>();
  /** AskUserQuestion tool calls: asked as question requests, so not shown as tool calls. */
  private questionCalls = new Set<string>();
  /** Requests the CLI withdrew before they were answered. */
  private abandoned = new Set<string>();
  /** Each MCP server's state as last reported, by name. */
  private mcp = new Map<string, McpServerStatus>();

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
      case "set_permission_mode":
        return [{ op: "set_permission_mode", mode: CLI_MODE[command.permission_mode], permission_mode: command.permission_mode }];
      case "answer_request":
        // The CLI gave up on it (the turn was interrupted): nothing is waiting for the answer.
        if (this.abandoned.delete(command.request_id)) return [];
        return [this.answer(command.request_id, command.verdict)];
      case "resume":
        // The CLI reloads the conversation itself (`--resume <id>`), so the agent keeps its context.
        return [{ op: "resume", session_id: command.external_session_id }];
    }
  }

  private system(msg: Json): ProtocolEvent[] {
    if (msg.subtype === "init") {
      // A re-sent init only says something new about MCP servers.
      if (this.initialisedSession === msg.session_id) return this.mcpChanges(msg.mcp_servers);
      this.initialisedSession = msg.session_id;
      const servers = mcpStatuses(msg.mcp_servers);
      this.mcp = new Map(servers.map((s) => [s.name, s]));
      return [
        {
          type: "session_init",
          external_session_id: msg.session_id,
          permission_mode: switchyardMode(msg.permissionMode),
          cli_permission_mode: msg.permissionMode,
          cli: { name: "claude-code", version: msg.claude_code_version },
          model: msg.model ?? null,
          mcp_servers: servers,
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
      else if (block.type === "tool_use" && block.name === QUESTION_TOOL) this.questionCalls.add(block.id);
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
      .filter((block: Json) => block.type === "tool_result" && !this.questionCalls.has(block.tool_use_id))
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
    if (msg.subtype === "can_use_tool") return [this.permissionRequest(msg)];
    if (msg.subtype === "can_use_tool_aborted" && this.pending.delete(msg.request_id)) this.abandoned.add(msg.request_id);
    if (msg.subtype === "mcp_status") return this.mcpChanges(msg.servers);
    return [];
  }

  /** True while a server reported at init has not settled (the driver asks the SDK again). */
  mcpPending(): boolean {
    return [...this.mcp.values()].some((s) => s.state === "pending");
  }

  /** An mcp_status event for the servers whose state differs from the last reported. */
  private mcpChanges(raw: unknown): ProtocolEvent[] {
    const changed = mcpStatuses(raw).filter((s) => {
      const known = this.mcp.get(s.name);
      return !known || known.state !== s.state || known.error !== s.error;
    });
    for (const s of changed) this.mcp.set(s.name, s);
    return changed.length ? [{ type: "mcp_status", servers: changed }] : [];
  }

  private permissionRequest(msg: Json): ProtocolEvent {
    const tool: string = msg.tool_name;
    const input: Json = msg.input ?? {};
    this.pending.set(msg.request_id, { tool, input });
    const reason = msg.decision_reason ?? (typeof input.description === "string" ? input.description : null);
    return { type: "permission_request", request_id: msg.request_id, request: requestDetail(tool, input, msg), reason };
  }

  private answer(requestId: string, verdict: Verdict): DriverOp {
    const request = this.pending.get(requestId);
    if (!request) throw new AdapterError(`no pending request ${requestId}`);
    this.pending.delete(requestId);
    const resolve = (result: PermissionResult): DriverOp => ({ op: "resolve_permission", request_id: requestId, result });
    switch (verdict.decision) {
      case "allow_once":
      case "allow_always": // The Grant is Switchyard's to keep; the CLI is only told to go ahead.
        return resolve({ behavior: "allow", updatedInput: request.input });
      case "deny":
        return resolve({ behavior: "deny", message: verdict.message || DECLINED });
      case "answer":
        if (request.tool === QUESTION_TOOL)
          return resolve({ behavior: "allow", updatedInput: answered(request.input, verdict.text) });
        if (request.tool === PLAN_TOOL) return resolve({ behavior: "deny", message: verdict.text });
        throw new AdapterError(`request ${requestId} is not a question`);
    }
  }
}

/** One of the five request kinds for a canUseTool call. */
function requestDetail(tool: string, input: Json, msg: Json): RequestDetail {
  const str = (v: unknown) => (typeof v === "string" ? v : null);
  if (tool === "Bash") return { kind: "command", command: str(input.command) ?? "", cwd: null };
  if (["Write", "Edit", "MultiEdit", "NotebookEdit"].includes(tool))
    return { kind: "file_write", path: str(input.file_path) ?? str(input.notebook_path) ?? "", diff: editDiff(input) };
  if (tool === "WebFetch") {
    const url = str(input.url);
    return { kind: "network", domain: hostOf(url), url };
  }
  if (tool.startsWith("mcp__")) {
    const [, server, ...rest] = tool.split("__");
    return { kind: "mcp_tool", server, tool: rest.join("__"), input };
  }
  if (tool === QUESTION_TOOL) {
    const questions: Json[] = Array.isArray(input.questions) ? input.questions : [];
    if (questions.length === 1)
      return {
        kind: "question",
        question: questions[0].question,
        options: (questions[0].options ?? []).map((o: Json) => o.label),
      };
    // Several questions are asked as one; the reply goes back as a free-form response.
    return { kind: "question", question: questions.map((q) => q.question).join("\n"), options: [] };
  }
  if (tool === PLAN_TOOL)
    return {
      kind: "question",
      question: `The agent proposes a plan and wants to stop exploring:\n\n${str(input.plan) ?? ""}`,
      options: ["Keep exploring"],
    };
  // Anything else the CLI asks about (a read outside the worktree, a web search, a
  // subagent…) is shown the way the CLI writes its own permission rules: Tool(argument).
  const argument =
    str(msg.blocked_path) ?? str(input.file_path) ?? str(input.path) ?? str(input.pattern) ?? str(input.query);
  return { kind: "command", command: argument ? `${tool}(${argument})` : tool, cwd: null };
}

function editDiff(input: Json): string | null {
  const prefixed = (prefix: string, text: string) => text.split("\n").map((l) => prefix + l);
  if (typeof input.old_string === "string" && typeof input.new_string === "string")
    return [...prefixed("-", input.old_string), ...prefixed("+", input.new_string)].join("\n");
  if (typeof input.content === "string") return prefixed("+", input.content).join("\n");
  return null;
}

function hostOf(url: string | null): string {
  try {
    return url ? new URL(url).hostname : "";
  } catch {
    return url ?? "";
  }
}

/** AskUserQuestion's input plus the user's reply: a single question gets it as its answer
 * (an option label or free text); several get it as one free-form response. */
function answered(input: Json, text: string): Json {
  const questions: Json[] = Array.isArray(input.questions) ? input.questions : [];
  if (questions.length === 1) return { ...input, answers: { [questions[0].question]: text } };
  return { ...input, response: text };
}

/** The SDK's MCP server statuses (system/init `mcp_servers`, `query.mcpServerStatus()`). */
function mcpStatuses(raw: unknown): McpServerStatus[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((s: Json) => typeof s?.name === "string")
    .map((s: Json) => ({
      name: s.name,
      state: MCP_STATE[s.status] ?? "failed",
      error: typeof s.error === "string" ? s.error : null,
    }));
}

const MCP_STATE: Record<string, McpServerState> = {
  connected: "connected",
  pending: "pending",
  failed: "failed",
  "needs-auth": "needs_auth",
  disabled: "disabled",
};

function resultText(content: unknown): string | null {
  if (typeof content === "string") return content;
  if (Array.isArray(content))
    return content
      .filter((c: Json) => c.type === "text")
      .map((c: Json) => c.text)
      .join("\n");
  return null;
}

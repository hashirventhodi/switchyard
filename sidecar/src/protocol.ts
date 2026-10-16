// Adapter protocol v0.2 wire types (the Rust types in crates/adapter-protocol/src/protocol.rs
// are the reference; see its README). Events and commands are tagged by `type`, snake_case.

export const PROTOCOL_RANGE = "^0.2";

export type PermissionMode = "explore" | "ask_first" | "auto_edit" | "full_auto" | "bypass";

export type RequestDetail =
  | { kind: "command"; command: string; cwd: string | null }
  | { kind: "file_write"; path: string; diff: string | null }
  | { kind: "network"; domain: string; url: string | null }
  | { kind: "mcp_tool"; server: string; tool: string; input: unknown }
  | { kind: "question"; question: string; options: string[] };

export type McpServerState = "connected" | "pending" | "failed" | "needs_auth" | "disabled";

/** An MCP server's connection state; `error` says why a failed one failed. */
export type McpServerStatus = { name: string; state: McpServerState; error: string | null };

export type ProtocolEvent =
  | {
      type: "session_init";
      external_session_id: string;
      permission_mode: PermissionMode;
      cli_permission_mode: string;
      cli: { name: string; version: string };
      model: string | null;
      mcp_servers: McpServerStatus[];
    }
  /** Servers whose state changed since last reported (the others are unchanged). */
  | { type: "mcp_status"; servers: McpServerStatus[] }
  | { type: "agent_text"; text: string }
  | { type: "tool_call_started"; call_id: string; tool: string; input: unknown }
  | { type: "tool_call_finished"; call_id: string; ok: boolean; output: string | null }
  | { type: "permission_request"; request_id: string; request: RequestDetail; reason: string | null }
  | { type: "turn_complete" }
  | { type: "done"; summary: string | null }
  | { type: "error"; message: string; fatal: boolean }
  /** Running totals on one Model (null: the Session's Model), superseding the last report for it. */
  | {
      type: "token_usage";
      model: string | null;
      input_tokens: number;
      output_tokens: number;
      cache_read_tokens: number;
      /** Left out when 0. */
      cache_write_tokens?: number;
      cost_usd: number | null;
    }
  | { type: "classifier_denial"; tool: string; detail: string; reason: string | null }
  /** The main agent's own requests now run on another Model (never a subagent's or background call's). */
  | { type: "model_changed"; model: string }
  /** A set_model did not take: the CLI refused it (its reason), or the next request was still on the old Model. */
  | { type: "model_change_failed"; model: string; reason: string | null };

export type Verdict =
  | { decision: "allow_once" }
  | { decision: "allow_always" }
  | { decision: "deny"; message?: string | null }
  | { decision: "answer"; text: string };

export type ProtocolCommand =
  | { type: "send_text"; text: string }
  | { type: "answer_request"; request_id: string; verdict: Verdict }
  | { type: "interrupt" }
  | { type: "set_permission_mode"; permission_mode: PermissionMode }
  | { type: "resume"; external_session_id: string }
  | { type: "set_model"; model: string };

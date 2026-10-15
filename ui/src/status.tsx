// The five session statuses, one colour and one label each, used by every surface.
import type { AgentKind } from "./schema/AgentKind";
import type { PermissionMode } from "./schema/PermissionMode";
import type { SessionStatus } from "./schema/SessionStatus";

export const STATUS_LABEL: Record<SessionStatus, string> = {
  running: "Running",
  waiting: "Needs you",
  idle: "Paused",
  done: "Finished",
  failed: "Error",
};

export const AGENT_LABEL: Record<AgentKind, string> = { claudeCode: "Claude Code", codex: "OpenAI Codex" };
export const AGENT_SHORT: Record<AgentKind, string> = { claudeCode: "CC", codex: "CX" };
/** Each agent's identity hue (prototype tokens `--ag-*`): avatars, chips, dock tabs. */
export const AGENT_COLOR: Record<AgentKind, string> = { claudeCode: "var(--ag-claude)", codex: "var(--ag-codex)" };
export const AGENTS: AgentKind[] = ["claudeCode", "codex"];

/** The agent's name with a swatch of its hue (the prototype's agent chip). */
export function AgentChip({ agent }: { agent: AgentKind }) {
  return (
    <span className="agent-chip" data-agent={agent}>
      <i style={{ background: AGENT_COLOR[agent] }} />
      {AGENT_LABEL[agent]}
    </span>
  );
}

export const MODE_LABEL: Record<PermissionMode, string> = {
  explore: "Explore",
  askFirst: "Ask first",
  autoEdit: "Auto edit",
  fullAuto: "Full auto",
};

/** Modes from least to most autonomous. */
const MODE_RANK: Record<PermissionMode, number> = { explore: 0, askFirst: 1, autoEdit: 2, fullAuto: 3 };

/** Whether `mode` is allowed under a repository's `ceiling` (none: every mode). */
export function withinCeiling(mode: PermissionMode, ceiling: PermissionMode | null): boolean {
  return ceiling === null || MODE_RANK[mode] <= MODE_RANK[ceiling];
}

/** The CLI started in another Permission mode than the one asked for. */
export function ModeWarning({ text }: { text: string }) {
  return (
    <div className="mode-warning" role="status" title={text}>
      ⚠ {text}
    </div>
  );
}

/** A status as a pill (session header) or a dot with its label (rows, tabs). */
export function Status({ status, variant = "pill" }: { status: SessionStatus; variant?: "pill" | "dot" }) {
  return (
    <span className={`status-${variant} st-${status}`} data-testid="status" data-status={status}>
      {STATUS_LABEL[status]}
    </span>
  );
}

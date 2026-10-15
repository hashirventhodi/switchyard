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

export const AGENT_LABEL: Record<AgentKind, string> = { claudeCode: "Claude Code" };
export const AGENT_SHORT: Record<AgentKind, string> = { claudeCode: "CC" };

export const MODE_LABEL: Record<PermissionMode, string> = {
  explore: "Explore",
  askFirst: "Ask first",
  autoEdit: "Auto edit",
  fullAuto: "Full auto",
};

/** A status as a pill (session header) or a dot with its label (rows, tabs). */
export function Status({ status, variant = "pill" }: { status: SessionStatus; variant?: "pill" | "dot" }) {
  return (
    <span className={`status-${variant} st-${status}`} data-testid="status" data-status={status}>
      {STATUS_LABEL[status]}
    </span>
  );
}

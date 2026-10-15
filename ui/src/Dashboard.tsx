// The Dashboard: KPIs, the sessions that need the user, and every session as a card.
// Everything is derived from the core's events; nothing polls.
import type { ReactNode } from "react";
import { at } from "./chunked";
import { wants } from "./requests";
import type { SessionStatus } from "./schema/SessionStatus";
import type { SessionView, State } from "./state";
import { PrBadge } from "./PullRequest";
import { AGENT_LABEL, Status } from "./status";

const ATTENTION: Record<SessionStatus, number> = { waiting: 0, failed: 1, running: 2, idle: 3, done: 4 };

export function Dashboard(props: {
  state: State;
  onOpenSession: (id: string) => void;
  onOpenInbox: () => void;
  /** The agent setup state, above the KPIs. */
  setup?: ReactNode;
  /** The Spend today KPI. */
  spend?: ReactNode;
}) {
  const { state, onOpenSession, onOpenInbox, setup, spend } = props;
  const sessions = Object.values(state.sessions);
  const count = (s: SessionStatus) => sessions.filter((x) => x.status === s).length;
  const sorted = [...sessions].sort((a, b) => ATTENTION[a.status] - ATTENTION[b.status]);
  const attention = sorted.filter((s) => s.status === "waiting" || s.status === "failed");
  const card = (s: SessionView) => <SessionCard key={s.id} session={s} state={state} onOpen={() => onOpenSession(s.id)} />;
  const kpis: [string, SessionStatus, (() => void) | undefined][] = [
    ["Running", "running", undefined],
    ["Needs you", "waiting", onOpenInbox],
    ["Errors", "failed", undefined],
  ];

  return (
    <div className="page">
      <div className="pagehead">
        <h1>Dashboard</h1>
        <span className="sub">
          {state.repositories.length} repositories · {sessions.length} sessions
        </span>
      </div>
      {setup}
      <div className="kpis">
        {kpis.map(([label, status, onClick]) => (
          <button key={label} className={`kpi ${status}`} aria-label={`${label}: ${count(status)}`} onClick={onClick}>
            <div className="l">{label}</div>
            <div className="v">{count(status)}</div>
          </button>
        ))}
        {spend}
      </div>
      {attention.length > 0 && (
        <section aria-label="Needs you">
          <div className="sec">
            <h2>Needs you</h2>
            <span className="sub">
              {count("waiting")} waiting · {count("failed")} error{count("failed") === 1 ? "" : "s"}
            </span>
          </div>
          <div className="cols">{attention.map(card)}</div>
        </section>
      )}
      <section aria-label="All sessions">
        <div className="sec">
          <h2>All sessions</h2>
          <span className="sub">sorted by attention</span>
        </div>
        {sessions.length ? <div className="cols">{sorted.map(card)}</div> : <p className="empty">No sessions yet. Start one with New session.</p>}
      </section>
    </div>
  );
}

/** What a session's card says it is doing: its pending request, else its last words. */
export function lastLine(session: SessionView, state: State): string {
  const pending = state.requests[session.id]?.[0];
  if (pending) return wants(pending.request);
  for (let i = session.transcript.length - 1; i >= 0; i--) {
    const entry = at(session.transcript, i);
    if (entry?.type === "agent") return entry.text;
    if (entry?.type === "tool" && entry.state === "failed") return `Failed: ${entry.summary}`;
  }
  return session.status === "waiting" ? "Waiting for your next prompt" : "";
}

function SessionCard(props: { session: SessionView; state: State; onOpen: () => void }) {
  const { session, state, onOpen } = props;
  const repo = state.repositories.find((r) => r.id === session.repositoryId);
  const name = session.branch ?? session.worktreePath;
  return (
    <div className={`card ${session.status}`}>
      <div className="top">
        <span className="branch">{name}</span>
        <Status status={session.status} />
      </div>
      <div className="proj">
        {repo?.name ?? ""} · {AGENT_LABEL[session.agent]}
      </div>
      <PrBadge state={state} worktreePath={session.worktreePath} />
      <div className="last">{lastLine(session, state)}</div>
      <div className="foot">
        <span>{session.facts.turns} turns</span>
        <span className="spacer" />
        <button className={`btn sm ${session.status === "waiting" ? "primary" : ""}`} aria-label={`Open ${name}`} onClick={onOpen}>
          {session.status === "waiting" ? "Respond" : session.status === "failed" ? "Recover" : "Open"}
        </button>
      </div>
    </div>
  );
}

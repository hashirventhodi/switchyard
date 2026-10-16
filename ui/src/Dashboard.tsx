// The Dashboard: KPIs, the sessions that need the user, and every session as a card.
// Everything is derived from the core's events; nothing polls.
import { useState, type ReactNode } from "react";
import { at } from "./chunked";
import { sessionTitle } from "./ChangesTab";
import type { NewSessionPreset } from "./NewSession";
import type { AgentKind } from "./schema/AgentKind";
import { wants } from "./requests";
import type { SessionStatus } from "./schema/SessionStatus";
import { sessionOn, type Activity, type SessionView, type State } from "./state";
import { PrBadge } from "./PullRequest";
import { AGENT_COLOR, AGENT_LABEL, AGENT_SHORT, AGENTS, AgentChip, ModeWarning, Status, STATUS_LABEL } from "./status";
import "./dashboard.css";

const ATTENTION: Record<SessionStatus, number> = { waiting: 0, failed: 1, running: 2, idle: 3, done: 4 };

export function Dashboard(props: {
  state: State;
  onOpenSession: (id: string) => void;
  onOpenInbox: () => void;
  /** The agent setup state, above the KPIs. */
  setup?: ReactNode;
  /** The Spend today KPI. */
  spend?: ReactNode;
  /** Open New session (on a free worktree from the occupancy strip). */
  onNewSession?: (preset: NewSessionPreset) => void;
}) {
  const { state, onOpenSession, onOpenInbox, setup, spend, onNewSession } = props;
  const [agent, setAgent] = useState<AgentKind | "all">("all");
  // Changed files counts what Changes would list, from the facts the core pushes (at attach
  // and on every change on disk).
  const sessions = Object.values(state.sessions);
  const worktrees = state.repositories.flatMap((r) => r.worktrees.map((w) => ({ repo: r, worktree: w })));
  const changedFiles = worktrees
    .filter(({ worktree }) => !worktree.primary)
    .reduce((n, { worktree }) => n + (state.worktreeFacts[worktree.path]?.changedFiles ?? 0), 0);
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
        <div className="kpi" aria-label={`Changed files: ${changedFiles}`}>
          <div className="l">Changed files</div>
          <div className="v">
            {changedFiles}
            <small>awaiting review</small>
          </div>
        </div>
        {spend}
      </div>
      {attention.length > 0 && (
        <section aria-label="Needs you">
          <div className="sec">
            <h2>Needs you</h2>
            <span className="sub">
              {count("waiting")} Needs you · {count("failed")} error{count("failed") === 1 ? "" : "s"}
            </span>
          </div>
          <div className="cols">{attention.map(card)}</div>
        </section>
      )}
      <section aria-label="All sessions">
        <div className="sec">
          <h2>All sessions</h2>
          <span className="sub">sorted by attention</span>
          <span className="r mode-sel" role="radiogroup" aria-label="Filter sessions by agent">
            {(["all", ...AGENTS] as const).map((k) => (
              <button key={k} role="radio" aria-checked={agent === k} className={agent === k ? "on" : ""} onClick={() => setAgent(k)}>
                {k === "all" ? "All" : AGENT_LABEL[k]}
              </button>
            ))}
          </span>
        </div>
        {sessions.length ? (
          <div className="cols">{sorted.filter((s) => agent === "all" || s.agent === agent).map(card)}</div>
        ) : (
          <p className="empty">No sessions yet. Start one with New session.</p>
        )}
      </section>
      {worktrees.length > 0 && (
        <section aria-label="Worktree occupancy">
          <div className="sec">
            <h2>Worktree occupancy</h2>
            <span className="sub">one agent per worktree keeps changes isolated</span>
          </div>
          <ul className="occ-list">
            {worktrees.map(({ repo, worktree }) => {
              const name = `${repo.name}/${worktree.branch ?? "detached"}`;
              const s = sessionOn(state, worktree.path);
              return (
                <li key={worktree.path} className="occ">
                  <span className="n" title={worktree.path}>
                    {name}
                  </span>
                  <div className="bar">
                    {s ? (
                      <button
                        className="seg"
                        style={{ background: AGENT_COLOR[s.agent], width: OCCUPIED_WIDTH[s.status] }}
                        onClick={() => onOpenSession(s.id)}
                      >
                        {AGENT_SHORT[s.agent]} · {sessionTitle(s)} · {STATUS_LABEL[s.status]}
                      </button>
                    ) : (
                      <button
                        className="seg free"
                        aria-label={`Start an agent in ${name}`}
                        onClick={() => onNewSession?.({ worktreePath: worktree.path })}
                      >
                        free · start an agent here
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        </section>
      )}
      {state.activity.length > 0 && (
        <section aria-label="Activity">
          <div className="sec">
            <h2>Activity</h2>
          </div>
          <ul className="feed">
            {state.activity.slice(0, FEED_LINES).map((a) => (
              <ActivityLine key={a.id} activity={a} />
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

const FEED_LINES = 9;

/** How full an occupant's bar is: done work fills it, a paused session barely. */
const OCCUPIED_WIDTH: Record<SessionView["status"], string> = {
  running: "72%",
  waiting: "72%",
  failed: "72%",
  idle: "38%",
  done: "100%",
};

/** "now", "3m ago", "2h ago", "4d ago". */
export function ago(ms: number): string {
  const m = Math.floor(ms / 60_000);
  if (m < 1) return "now";
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h}h ago` : `${Math.floor(h / 24)}d ago`;
}

function ActivityLine({ activity: a }: { activity: Activity }) {
  return (
    <li className="ev">
      <span className="tm">{ago(Date.now() - a.atMs)}</span>
      <span className={`d ${a.tone ?? ""}`} />
      <span>
        {a.agent && <b>{AGENT_LABEL[a.agent]}</b>}
        {a.agent && " · "}
        <code>{a.branch}</code>: {a.text}
      </span>
    </li>
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
        {repo?.name ?? ""} · <AgentChip agent={session.agent} />
      </div>
      <PrBadge state={state} worktreePath={session.worktreePath} />
      {session.facts.modeWarning && <ModeWarning text={session.facts.modeWarning} />}
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

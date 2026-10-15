// Renderer state is derived only from core events.
import { emptyChunked, put, type Chunked } from "./chunked";
import type { AgentSetup } from "./schema/AgentSetup";
import type { Entry } from "./schema/Entry";
import type { Event } from "./schema/Event";
import type { Repository } from "./schema/Repository";
import type { Session } from "./schema/Session";
import type { SessionFacts } from "./schema/SessionFacts";
import type { Worktree } from "./schema/Worktree";

export interface RepositoryView extends Repository {
  worktrees: Worktree[];
}

export interface SessionView extends Session {
  facts: SessionFacts;
  transcript: Chunked<Entry>;
}

export interface Toast {
  id: number;
  tone: "bad" | "ok" | "info" | "warn";
  text: string;
}

export interface State {
  repositories: RepositoryView[];
  /** By id, in start order. */
  sessions: Record<string, SessionView>;
  toasts: Toast[];
  /** What the core detected about each agent CLI; empty until it reports. */
  agents: AgentSetup[];
}

export const initialState: State = { repositories: [], sessions: {}, toasts: [], agents: [] };

const noFacts: SessionFacts = { model: null, cliVersion: null, cliPermissionMode: null, turns: 0, costUsd: null };

let toastSeq = 0;

function updateSession(state: State, id: string, change: (s: SessionView) => SessionView): State {
  const current = state.sessions[id];
  return current ? { ...state, sessions: { ...state.sessions, [id]: change(current) } } : state;
}

export function reduce(state: State, event: Event): State {
  switch (event.type) {
    case "attached":
      return { ...state, repositories: [], sessions: {} };
    case "repositoryAdded": {
      const others = state.repositories.filter((r) => r.id !== event.repository.id);
      return { ...state, repositories: [...others, { ...event.repository, worktrees: [] }] };
    }
    case "worktreeAdded":
      return {
        ...state,
        repositories: state.repositories.map((r) =>
          r.id === event.repositoryId
            ? { ...r, worktrees: [...r.worktrees.filter((w) => w.path !== event.worktree.path), event.worktree] }
            : r,
        ),
      };
    case "commandRefused":
      return { ...state, toasts: [...state.toasts, { id: ++toastSeq, tone: "bad", text: event.reason }] };
    case "sessionStarted": {
      const s = event.session;
      return { ...state, sessions: { ...state.sessions, [s.id]: { ...s, facts: noFacts, transcript: emptyChunked() } } };
    }
    case "sessionStatusChanged":
      return updateSession(state, event.sessionId, (s) => ({ ...s, status: event.status }));
    case "sessionFactsChanged":
      return updateSession(state, event.sessionId, (s) => ({ ...s, facts: event.facts }));
    case "transcriptEntry":
      return updateSession(state, event.sessionId, (s) => ({
        ...s,
        transcript: put(s.transcript, event.index, event.entry),
      }));
    case "agentsChecked":
      return { ...state, agents: event.agents };
  }
}

export function dismissToast(state: State, id: number): State {
  return { ...state, toasts: state.toasts.filter((t) => t.id !== id) };
}

/** The session occupying a worktree, if any (at most one, ADR-0005). */
export function sessionOn(state: State, worktreePath: string): SessionView | undefined {
  return Object.values(state.sessions).find((s) => s.worktreePath === worktreePath);
}

// Renderer state is derived only from core events.
import { emptyChunked, put, type Chunked } from "./chunked";
import type { AgentSetup } from "./schema/AgentSetup";
import type { Entry } from "./schema/Entry";
import type { Event } from "./schema/Event";
import type { Repository } from "./schema/Repository";
import type { Session } from "./schema/Session";
import type { SessionFacts } from "./schema/SessionFacts";
import type { Shell } from "./schema/Shell";
import type { SpendSample } from "./spend";
import type { Worktree } from "./schema/Worktree";
import type { WorktreeFacts } from "./schema/WorktreeFacts";

export interface RepositoryView extends Repository {
  worktrees: Worktree[];
}

export interface SessionView extends Session {
  facts: SessionFacts;
  transcript: Chunked<Entry>;
  /** Raw adapter-protocol events (JSON), for the Agent log's debug view. */
  rawEvents: Chunked<string>;
  /** Running spend totals as recorded, oldest first. */
  spend: SpendSample[];
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
  /** Open shell terminals, in open order. */
  shells: Shell[];
  toasts: Toast[];
  /** What the core detected about each agent CLI; empty until it reports. */
  agents: AgentSetup[];
  /** What the core last read from the VCS about each worktree, by path. */
  worktreeFacts: Record<string, WorktreeFacts>;
}

export const initialState: State = { repositories: [], sessions: {}, shells: [], toasts: [], agents: [], worktreeFacts: {} };

const noFacts: SessionFacts = { model: null, cliVersion: null, cliPermissionMode: null, turns: 0 };

let toastSeq = 0;

function updateSession(state: State, id: string, change: (s: SessionView) => SessionView): State {
  const current = state.sessions[id];
  return current ? { ...state, sessions: { ...state.sessions, [id]: change(current) } } : state;
}

export function reduce(state: State, event: Event): State {
  switch (event.type) {
    case "attached":
      return { ...state, repositories: [], sessions: {}, shells: [] };
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
    case "worktreeRemoved": {
      const repo = state.repositories.find((r) => r.id === event.repositoryId);
      const gone = repo?.worktrees.find((w) => w.path === event.worktreePath);
      const { [event.worktreePath]: _dropped, ...worktreeFacts } = state.worktreeFacts;
      const text = `Removed worktree ${gone?.branch ?? event.worktreePath}`;
      return {
        ...state,
        worktreeFacts,
        repositories: state.repositories.map((r) =>
          r.id === event.repositoryId ? { ...r, worktrees: r.worktrees.filter((w) => w.path !== event.worktreePath) } : r,
        ),
        toasts: [...state.toasts, { id: ++toastSeq, tone: "ok", text }],
      };
    }
    case "worktreeFactsChanged":
      return { ...state, worktreeFacts: { ...state.worktreeFacts, [event.worktreePath]: event.facts } };
    case "commandRefused":
      return { ...state, toasts: [...state.toasts, { id: ++toastSeq, tone: "bad", text: event.reason }] };
    case "sessionStarted": {
      const s = event.session;
      const view = { ...s, facts: noFacts, transcript: emptyChunked<Entry>(), rawEvents: emptyChunked<string>(), spend: [] };
      return { ...state, sessions: { ...state.sessions, [s.id]: view } };
    }
    case "sessionStatusChanged":
      return updateSession(state, event.sessionId, (s) => ({ ...s, status: event.status }));
    case "sessionFactsChanged":
      return updateSession(state, event.sessionId, (s) => ({ ...s, facts: event.facts }));
    case "spendRecorded":
      return updateSession(state, event.sessionId, (s) => ({
        ...s,
        spend: [...s.spend, { atMs: event.atMs, spend: event.spend }],
      }));
    case "transcriptEntry":
      return updateSession(state, event.sessionId, (s) => ({
        ...s,
        transcript: put(s.transcript, event.index, event.entry),
      }));
    case "agentEventReceived":
      return updateSession(state, event.sessionId, (s) => ({
        ...s,
        rawEvents: put(s.rawEvents, event.index, event.json),
      }));
    case "shellOpened":
      return { ...state, shells: [...state.shells.filter((s) => s.id !== event.shell.id), event.shell] };
    case "shellClosed":
      return { ...state, shells: state.shells.filter((s) => s.id !== event.shellId) };
    case "shellOutput":
      // A byte stream for the terminal emulator (see shellStreams.ts), not renderer state.
      return state;
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

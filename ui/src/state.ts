// Renderer state is derived only from core events.
import { emptyChunked, put, type Chunked } from "./chunked";
import type { AgentSetup } from "./schema/AgentSetup";
import type { Entry } from "./schema/Entry";
import type { Policy } from "./schema/Policy";
import type { Event } from "./schema/Event";
import type { Repository } from "./schema/Repository";
import type { Request } from "./schema/Request";
import { detail, verb } from "./requests";
import { AGENT_LABEL } from "./status";
import type { Session } from "./schema/Session";
import type { SessionFacts } from "./schema/SessionFacts";
import type { Shell } from "./schema/Shell";
import type { SpendSample } from "./spend";
import type { Worktree } from "./schema/Worktree";
import type { WorktreeFacts } from "./schema/WorktreeFacts";
import type { Commit } from "./schema/Commit";

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

/** A Permission request waiting for the user. */
export interface PendingRequest {
  sessionId: string;
  requestId: string;
  /** Its transcript index. */
  index: number;
  request: Request;
}

/** How long a toast stays: a Permission request 20 s, the result of a user action 4 s. */
export const REQUEST_TOAST_MS = 20_000;
export const RESULT_TOAST_MS = 4_000;

export interface Toast {
  id: number;
  tone: "bad" | "ok" | "info" | "warn";
  text: string;
  ms: number;
  /** Mono second line: the exact command. */
  detail?: string;
  /** The Permission request the toast offers to answer; it goes when the request does. */
  request?: PendingRequest;
}

export interface State {
  repositories: RepositoryView[];
  /** By id, in start order. */
  sessions: Record<string, SessionView>;
  /** Open shell terminals, in open order. */
  shells: Shell[];
  /** Pending Permission requests by session, in transcript order. */
  requests: Record<string, PendingRequest[]>;
  toasts: Toast[];
  /** What the core detected about each agent CLI; empty until it reports. */
  agents: AgentSetup[];
  /** What the core last read from the VCS about each worktree, by path. */
  worktreeFacts: Record<string, WorktreeFacts>;
  /** The commits each worktree's branch has above its base, by path, as last read. */
  history: Record<string, WorktreeHistory>;
  /** Each Repository's resolved Policy, by repository id, once the core has sent it. */
  policies: Record<string, Policy>;
}

export interface WorktreeHistory {
  base: string;
  /** Newest first. */
  commits: Commit[];
}

export const initialState: State = {
  repositories: [],
  sessions: {},
  shells: [],
  requests: {},
  toasts: [],
  agents: [],
  worktreeFacts: {},
  history: {},
  policies: {},
};

const noFacts: SessionFacts = { model: null, cliVersion: null, cliPermissionMode: null, turns: 0 };

let toastSeq = 0;

function updateSession(state: State, id: string, change: (s: SessionView) => SessionView): State {
  const current = state.sessions[id];
  return current ? { ...state, sessions: { ...state.sessions, [id]: change(current) } } : state;
}

export function reduce(state: State, event: Event): State {
  switch (event.type) {
    case "attached":
      return {
        ...state,
        repositories: [],
        sessions: {},
        shells: [],
        requests: {},
        toasts: state.toasts.filter((t) => !t.request),
      };
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
      return resultToast(
        {
          ...state,
          worktreeFacts,
          repositories: state.repositories.map((r) =>
            r.id === event.repositoryId ? { ...r, worktrees: r.worktrees.filter((w) => w.path !== event.worktreePath) } : r,
          ),
        },
        text,
        "ok",
      );
    }
    case "worktreeFactsChanged":
      return { ...state, worktreeFacts: { ...state.worktreeFacts, [event.worktreePath]: event.facts } };
    case "worktreeHistory":
      return { ...state, history: { ...state.history, [event.worktreePath]: { base: event.base, commits: event.commits } } };
    case "worktreeRebased": {
      const worktree = state.repositories.flatMap((r) => r.worktrees).find((w) => w.path === event.worktreePath);
      return resultToast(state, `Rebased ${worktree?.branch ?? event.worktreePath} on ${event.base}`, "ok");
    }
    case "commandRefused":
      return addToast(state, { tone: "bad", text: event.reason, ms: RESULT_TOAST_MS });
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
    case "transcriptEntry": {
      const next = updateSession(state, event.sessionId, (s) => ({
        ...s,
        transcript: put(s.transcript, event.index, event.entry),
      }));
      return event.entry.type === "request" ? trackRequest(next, event.sessionId, event.index, event.entry) : next;
    }
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
    case "policyChanged":
      return { ...state, policies: { ...state.policies, [event.repositoryId]: event.policy } };
  }
}

/** Keep the pending list (and the request's toast) in step with a request card. */
function trackRequest(state: State, sessionId: string, index: number, entry: Entry & { type: "request" }): State {
  const list = state.requests[sessionId] ?? [];
  const known = list.some((r) => r.requestId === entry.requestId);
  if (entry.state === "pending") {
    if (known) return state;
    const pending: PendingRequest = { sessionId, requestId: entry.requestId, index, request: entry.request };
    const requests = { ...state.requests, [sessionId]: [...list, pending].sort((a, b) => a.index - b.index) };
    const session = state.sessions[sessionId];
    const who = session ? `${AGENT_LABEL[session.agent]} on ${session.branch ?? session.worktreePath}` : "An agent";
    return addToast(
      { ...state, requests },
      { tone: "warn", text: `${who} ${verb(entry.request)}`, detail: detail(entry.request), ms: REQUEST_TOAST_MS, request: pending },
    );
  }
  if (!known) return state;
  const gone = (r?: PendingRequest) => r?.sessionId === sessionId && r.requestId === entry.requestId;
  return {
    ...state,
    requests: { ...state.requests, [sessionId]: list.filter((r) => !gone(r)) },
    toasts: state.toasts.filter((t) => !gone(t.request)),
  };
}

function addToast(state: State, toast: Omit<Toast, "id">): State {
  return { ...state, toasts: [...state.toasts, { id: ++toastSeq, ...toast }] };
}

/** A toast for the result of something the user just did (4 s). */
export function resultToast(state: State, text: string, tone: Toast["tone"] = "info"): State {
  return addToast(state, { tone, text, ms: RESULT_TOAST_MS });
}

export function dismissToast(state: State, id: number): State {
  return { ...state, toasts: state.toasts.filter((t) => t.id !== id) };
}

/** Sessions that need the user, in start order. */
export function waitingSessions(state: State): SessionView[] {
  return Object.values(state.sessions).filter((s) => s.status === "waiting");
}

/** The session occupying a worktree, if any (at most one, ADR-0005). */
export function sessionOn(state: State, worktreePath: string): SessionView | undefined {
  return Object.values(state.sessions).find((s) => s.worktreePath === worktreePath);
}

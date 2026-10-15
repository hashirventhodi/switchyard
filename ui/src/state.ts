// Renderer state is derived only from core events.
import { emptyChunked, put, type Chunked } from "./chunked";
import { emptyRawLog, pushRaw, type RawLog } from "./rawLog";
import type { AgentSetup } from "./schema/AgentSetup";
import type { ArchivedSession } from "./schema/ArchivedSession";
import type { Settings } from "./schema/Settings";
import type { ChangedFile } from "./schema/ChangedFile";
import type { ConflictFile } from "./schema/ConflictFile";
import type { ContextFile } from "./schema/ContextFile";
import type { DeclaredMcpServer } from "./schema/DeclaredMcpServer";
import type { Entry } from "./schema/Entry";
import type { Policy } from "./schema/Policy";
import type { Secret } from "./schema/Secret";
import type { Event } from "./schema/Event";
import type { Repository } from "./schema/Repository";
import type { Request } from "./schema/Request";
import { detail, verb } from "./requests";
import { AGENT_LABEL } from "./status";
import type { AgentKind } from "./schema/AgentKind";
import type { Session } from "./schema/Session";
import type { SessionStatus } from "./schema/SessionStatus";
import type { SessionFacts } from "./schema/SessionFacts";
import type { Shell } from "./schema/Shell";
import type { SpendSample } from "./spend";
import type { Worktree } from "./schema/Worktree";
import type { WorktreeFacts } from "./schema/WorktreeFacts";
import type { Commit } from "./schema/Commit";
import type { ForgeSetup } from "./schema/ForgeSetup";
import type { PullRequest } from "./schema/PullRequest";
import type { Task } from "./schema/Task";

export interface RepositoryView extends Repository {
  worktrees: Worktree[];
}

export interface SessionView extends Session {
  facts: SessionFacts;
  transcript: Chunked<Entry>;
  /** Raw adapter-protocol events (JSON), for the Agent log's debug view. */
  rawEvents: RawLog;
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
  /** Uncommitted changes the core last read for each worktree, by path. */
  changes: Record<string, ChangedFile[]>;
  /** Each worktree's conflicts with their hunks, by path, as last read with its changes. */
  conflicts: Record<string, WorktreeConflicts>;
  /** The commits each worktree's branch has above its base, by path, as last read. */
  history: Record<string, WorktreeHistory>;
  /** Each Repository's resolved Policy, by repository id, once the core has sent it. */
  policies: Record<string, Policy>;
  /** Each Repository's secrets (names and flags, never values), by repository id, once read. */
  secrets: Record<string, Secret[]>;
  /** Archived Sessions by repository id, oldest first. */
  archived: Record<string, ArchivedSession[]>;
  /** The archived transcript last read (the open reader), if any. */
  archivedTranscript: { sessionId: string; entries: Entry[] } | null;
  /** The user settings the core keeps; null until it reports them. */
  settings: Settings | null;
  /** Whether pull requests can be opened (gh installed, signed in); null until the core reports. */
  forge: ForgeSetup | null;
  /** The pull request for each worktree's branch, by worktree path, as last polled. */
  pullRequests: Record<string, PullRequest>;
  /** When the Forge was last asked about each worktree's pull request (ms), by worktree path. */
  pullRequestChecked: Record<string, number>;
  /** Each Repository's Tasks from every source, by repository id, as last read. */
  tasks: Record<string, Task[]>;
  /** The house rules a task's prompt carries, by repository id. */
  houseRules: Record<string, string>;
  /** When each Repository's GitHub Issues were last read (ms); null when never, by id. */
  tasksChecked: Record<string, number | null>;
  /** The Task each Session (live or archived) is linked to, by session id. */
  taskLinks: Record<string, string>;
  /** Each Repository's declared MCP servers (its .mcp.json) and which are enabled, by id. */
  mcpDeclared: Record<string, { servers: DeclaredMcpServer[]; error: string | null }>;
  /** Each worktree's context files (instructions and agent memory), by path, as last read. */
  contextFiles: Record<string, ContextFile[]>;
  /** What happened, newest first (at most ACTIVITY_KEEP): the Dashboard's activity feed. */
  activity: Activity[];
}

/** One line of the activity feed. */
export interface Activity {
  id: string;
  atMs: number;
  /** Who, when a session did it. */
  agent: AgentKind | null;
  branch: string;
  text: string;
  /** Dot colour: a session status, or none. */
  tone: SessionStatus | null;
}

export const ACTIVITY_KEEP = 30;

const ACTIVITY_TEXT: Record<SessionStatus, string> = {
  running: "running",
  waiting: "needs you",
  idle: "paused",
  done: "finished",
  failed: "error",
};

/** Add `a` in time order (newest first); a line with the same id is kept once. */
function addActivity(state: State, a: Activity): State {
  if (state.activity.some((x) => x.id === a.id)) return state;
  const activity = [...state.activity, a].sort((x, y) => y.atMs - x.atMs).slice(0, ACTIVITY_KEEP);
  return { ...state, activity };
}

let activitySeq = 0;

function branchAt(state: State, path: string): string {
  return state.repositories.flatMap((r) => r.worktrees).find((w) => w.path === path)?.branch ?? path;
}

function sessionActivity(state: State, sessionId: string, text: string, tone: SessionStatus | null): State {
  const s = state.sessions[sessionId];
  if (!s) return state;
  return addActivity(state, {
    id: `a${++activitySeq}`,
    atMs: Date.now(),
    agent: s.agent,
    branch: s.branch ?? s.worktreePath,
    text,
    tone,
  });
}

function worktreeActivity(state: State, path: string, text: string): State {
  return addActivity(state, { id: `a${++activitySeq}`, atMs: Date.now(), agent: null, branch: branchAt(state, path), text, tone: null });
}

export interface WorktreeConflicts {
  base: string;
  files: ConflictFile[];
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
  changes: {},
  history: {},
  policies: {},
  secrets: {},
  archived: {},
  archivedTranscript: null,
  settings: null,
  forge: null,
  pullRequests: {},
  pullRequestChecked: {},
  tasks: {},
  houseRules: {},
  tasksChecked: {},
  taskLinks: {},
  conflicts: {},
  activity: [],
  contextFiles: {},
  mcpDeclared: {},
};

const noFacts: SessionFacts = {
  model: null,
  cliVersion: null,
  cliPermissionMode: null,
  turns: 0,
  modeWarning: null,
  classifierDenials: 0,
  mcpServers: [],
};

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
        archived: {},
        archivedTranscript: null,
        pullRequests: {},
        taskLinks: {},
        conflicts: {},
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
      const { [event.worktreePath]: _pr, ...pullRequests } = state.pullRequests;
      const text = `Removed worktree ${gone?.branch ?? event.worktreePath}`;
      return resultToast(
        {
          ...state,
          worktreeFacts,
          pullRequests,
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
    case "mcpServersChanged":
      return { ...state, mcpDeclared: { ...state.mcpDeclared, [event.repositoryId]: { servers: event.servers, error: event.error } } };
    case "contextRead":
      return { ...state, contextFiles: { ...state.contextFiles, [event.worktreePath]: event.files } };
    case "worktreeHistory":
      return { ...state, history: { ...state.history, [event.worktreePath]: { base: event.base, commits: event.commits } } };
    case "worktreeRebased": {
      const worktree = state.repositories.flatMap((r) => r.worktrees).find((w) => w.path === event.worktreePath);
      return resultToast(state, `Rebased ${worktree?.branch ?? event.worktreePath} on ${event.base}`, "ok");
    }
    case "rebaseStopped": {
      const branch = branchAt(state, event.worktreePath);
      const files = event.conflicts.length ? ` on conflicts in ${event.conflicts.join(", ")}` : "";
      return resultToast(
        state,
        `Rebasing ${branch} on ${event.base} stopped${files}: resolve them in Changes, then Rebase again to continue`,
        "warn",
      );
    }
    case "commandRefused":
      return addToast(state, { tone: "bad", text: event.reason, ms: RESULT_TOAST_MS });
    case "sessionStarted": {
      const s = event.session;
      const view = { ...s, facts: noFacts, transcript: emptyChunked<Entry>(), rawEvents: emptyRawLog(), spend: [] };
      const started = { ...state, sessions: { ...state.sessions, [s.id]: view } };
      // Replayed on every attach: the start time makes it the same line each time.
      return addActivity(started, {
        id: `started:${s.id}`,
        atMs: s.startedAtMs,
        agent: s.agent,
        branch: s.branch ?? s.worktreePath,
        text: "started",
        tone: "running",
      });
    }
    case "sessionStatusChanged":
      return sessionActivity(
        updateSession(state, event.sessionId, (s) => ({ ...s, status: event.status })),
        event.sessionId,
        ACTIVITY_TEXT[event.status],
        event.status,
      );
    case "sessionModeChanged":
      return updateSession(state, event.sessionId, (s) => ({ ...s, permissionMode: event.permissionMode }));
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
        rawEvents: pushRaw(s.rawEvents, event.index, event.json),
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
    case "changesRead":
      return { ...state, changes: { ...state.changes, [event.worktreePath]: event.files } };
    case "conflictsRead":
      return { ...state, conflicts: { ...state.conflicts, [event.worktreePath]: { base: event.base, files: event.files } } };
    case "conflictResolved": {
      const worktree = state.repositories.flatMap((r) => r.worktrees).find((w) => w.path === event.worktreePath);
      const session = Object.values(state.sessions).find((s) => s.worktreePath === event.worktreePath);
      const base = state.conflicts[event.worktreePath]?.base ?? "base";
      const kept = event.side === "base" ? base : (worktree?.branch ?? session?.branch ?? "the branch");
      const n = event.remaining;
      return n === 0
        ? resultToast(state, `Resolved ${event.path}, keeping ${kept}`, "ok")
        : resultToast(state, `Kept ${kept} in ${event.path} · ${n} conflict${n === 1 ? "" : "s"} left`, "ok");
    }
    case "fileDecided": {
      const list = state.changes[event.worktreePath];
      if (!list) return state;
      const next = list.map((f) => (f.path === event.path ? { ...f, decision: event.decision } : f));
      return { ...state, changes: { ...state.changes, [event.worktreePath]: next } };
    }
    case "hunkReviewed": {
      const list = state.changes[event.worktreePath];
      if (!list) return state;
      const { decision, comment, sent } = event;
      const next = list.map((f) =>
        f.path === event.path ? { ...f, hunks: f.hunks.map((h) => (h.id === event.hunkId ? { ...h, decision, comment, sent } : h)) } : f,
      );
      return { ...state, changes: { ...state.changes, [event.worktreePath]: next } };
    }
    case "feedbackSent": {
      const n = event.hunks;
      return resultToast(state, `Sent feedback on ${n} hunk${n === 1 ? "" : "s"} to the agent`, "ok");
    }
    case "committed": {
      const n = event.files.length;
      const text = `Committed ${n} file${n === 1 ? "" : "s"} to ${event.branch ?? "the worktree"} (${event.commit})`;
      const noted = worktreeActivity(state, event.worktreePath, `committed ${n} file${n === 1 ? "" : "s"} (${event.commit})`);
      return resultToast(noted, text, "ok");
    }
    case "sessionArchived": {
      const { session } = event.archived;
      const { [session.id]: _gone, ...sessions } = state.sessions;
      const { [session.id]: _requests, ...requests } = state.requests;
      const list = (state.archived[session.repositoryId] ?? []).filter((a) => a.session.id !== session.id);
      return {
        ...state,
        sessions,
        requests,
        toasts: state.toasts.filter((t) => t.request?.sessionId !== session.id),
        archived: { ...state.archived, [session.repositoryId]: [...list, event.archived] },
      };
    }
    case "archivedSessionPruned": {
      const list = (state.archived[event.repositoryId] ?? []).filter((a) => a.session.id !== event.sessionId);
      const reader = state.archivedTranscript?.sessionId === event.sessionId ? null : state.archivedTranscript;
      return { ...state, archived: { ...state.archived, [event.repositoryId]: list }, archivedTranscript: reader };
    }
    case "archivedTranscript":
      return { ...state, archivedTranscript: { sessionId: event.sessionId, entries: event.entries } };
    case "transcriptExported":
      return resultToast(state, `Exported the transcript to ${event.path}`, "ok");
    case "settingsChanged":
      return { ...state, settings: event.settings };
    case "policyChanged":
      return { ...state, policies: { ...state.policies, [event.repositoryId]: event.policy } };
    case "secretsChanged":
      return { ...state, secrets: { ...state.secrets, [event.repositoryId]: event.secrets } };
    case "forgeChecked":
      return { ...state, forge: event.forge };
    case "pullRequestChanged": {
      const { [event.worktreePath]: _old, ...others } = state.pullRequests;
      const pullRequests = event.pullRequest ? { ...others, [event.worktreePath]: event.pullRequest } : others;
      const pullRequestChecked = { ...state.pullRequestChecked, [event.worktreePath]: event.checkedAtMs };
      return { ...state, pullRequests, pullRequestChecked };
    }
    case "pullRequestOpened": {
      const branch = branchAt(state, event.worktreePath);
      const noted = worktreeActivity(state, event.worktreePath, `opened PR #${event.pullRequest.number}`);
      return resultToast(noted, `Opened PR #${event.pullRequest.number} for ${branch}`, "ok");
    }
    case "branchMerged": {
      const { branch, base, pullRequest, removed, kept } = event;
      const what = pullRequest === null ? `Merged ${branch} into ${base}` : `PR #${pullRequest} merged`;
      // One toast for the whole cleanup, not one for the merge and one for the removal.
      const quiet = addActivity(withoutRemovedToasts(state, [branch]), {
        id: `a${++activitySeq}`,
        atMs: Date.now(),
        agent: null,
        branch,
        text: pullRequest === null ? `merged into ${base}` : `PR #${pullRequest} merged`,
        tone: "done",
      });
      return removed
        ? resultToast(quiet, `${what} · removed worktree ${branch}`, "ok")
        : resultToast(quiet, `${what} · kept ${branch}: ${kept ?? "it could not be removed"}`, "warn");
    }
    case "mergedPruned": {
      const n = event.branches.length;
      const quiet = withoutRemovedToasts(state, event.branches);
      return n === 0
        ? resultToast(quiet, "No merged worktrees to prune")
        : resultToast(quiet, `Pruned ${n} merged worktree${n === 1 ? "" : "s"}: ${event.branches.join(", ")}`, "ok");
    }
    case "tasksChanged":
      return {
        ...state,
        tasks: { ...state.tasks, [event.repositoryId]: event.tasks },
        houseRules: { ...state.houseRules, [event.repositoryId]: event.houseRules },
        tasksChecked: { ...state.tasksChecked, [event.repositoryId]: event.checkedAtMs },
      };
    case "taskCreated":
      return resultToast(state, `Added ${event.task.key} to To do`, "ok");
    case "taskLinked": {
      const { [event.sessionId]: _old, ...others } = state.taskLinks;
      return { ...state, taskLinks: event.taskId ? { ...others, [event.sessionId]: event.taskId } : others };
    }
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

/** Drop the "Removed worktree" toasts a merge cleanup or prune is about to sum up. */
function withoutRemovedToasts(state: State, branches: string[]): State {
  const texts = new Set(branches.map((b) => `Removed worktree ${b}`));
  return { ...state, toasts: state.toasts.filter((t) => t.request || !texts.has(t.text)) };
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

/** A Task by id, from any repository's last-read list. */
export function taskById(state: State, id: string | undefined): Task | undefined {
  if (!id) return undefined;
  return Object.values(state.tasks).flat().find((t) => t.id === id);
}

/** The live Sessions linked to a Task, in start order. */
export function sessionsOfTask(state: State, taskId: string): SessionView[] {
  return Object.values(state.sessions).filter((s) => state.taskLinks[s.id] === taskId);
}

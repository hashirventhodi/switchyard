// The Tasks view: every Repository's Tasks on a four-column board (To do, In progress, In
// review, Done), the New local task modal, Assign to agent, and linking a task to a session
// ("Pick one" in Context).
import { useEffect, useState } from "react";
import { useAction } from "./actions";
import type { Command } from "./schema/Command";
import type { Task } from "./schema/Task";
import type { TaskKind } from "./schema/TaskKind";
import { sessionsOfTask, type SessionView, type State } from "./state";
import { AGENT_SHORT, Status } from "./status";
import { COLUMNS, SOURCE_LABEL } from "./tasks";

/** A session waiting for the user to pick its task; `relink` when it already has one. */
export interface Linking {
  sessionId: string;
  relink: boolean;
}

export function TasksView(props: {
  state: State;
  send: (command: Command) => void;
  onAssign: (task: Task) => void;
  onOpenSession: (id: string) => void;
  onNewTask: () => void;
  linking: Linking | null;
  onCancelLinking: () => void;
}) {
  const { state, send, onNewTask, linking, onCancelLinking } = props;
  const repoIds = state.repositories.map((r) => r.id).join("\n");
  useEffect(() => {
    for (const id of repoIds.split("\n").filter(Boolean)) send({ type: "readTasks", repositoryId: id });
  }, [send, repoIds]);
  useAction("confirm.cancel", onCancelLinking, !!linking);

  const tasks = state.repositories.flatMap((r) => state.tasks[r.id] ?? []);
  const linkingSession = linking ? state.sessions[linking.sessionId] : undefined;
  const repoName = (id: string) => state.repositories.find((r) => r.id === id)?.name ?? "";
  // GitHub Issues come only from repositories with a remote, through gh.
  const tracked = state.repositories.filter((r) => r.remote);
  const forge = state.forge?.state;
  const ghProblem =
    tracked.length === 0 || !forge || forge.type === "ready"
      ? null
      : forge.type === "missing"
        ? `GitHub CLI (gh) is not installed; install it (${forge.installHint}) to sync GitHub Issues.`
        : `GitHub CLI (gh) is not signed in; run ${forge.loginHint} to sync GitHub Issues.`;

  return (
    <div className="page">
      <div className="pagehead">
        <h1>Tasks</h1>
        <span className="sub">
          Local tasks and GitHub Issues per repository. Assign a task to an agent and a worktree is created for it.
        </span>
        <div className="right">
          <button
            className="btn"
            disabled={tracked.length === 0}
            title={tracked.length === 0 ? "No repository has a remote to read GitHub Issues from" : "Read GitHub Issues again"}
            onClick={() => tracked.forEach((r) => send({ type: "syncTasks", repositoryId: r.id }))}
          >
            Sync
          </button>
          <button className="btn" onClick={onNewTask} disabled={state.repositories.length === 0}>
            New local task
          </button>
        </div>
      </div>
      {ghProblem && (
        <div className="linking" role="status" aria-label="GitHub Issues">
          <span>{ghProblem}</span>
        </div>
      )}
      {linkingSession && (
        <div className="linking" role="status" aria-label="Linking">
          <span>
            Pick the task for <code>{linkingSession.branch ?? linkingSession.worktreePath}</code>
            {linking?.relink && "; it replaces the task linked now"}.
          </span>
          <button className="btn sm ghost" onClick={onCancelLinking}>
            Cancel
          </button>
        </div>
      )}
      {state.repositories.length === 0 && <p className="empty">No repositories yet. Add one with + Repo.</p>}
      <div className="tboard-wrap">
        <div className="tboard">
          {COLUMNS.map(([column, label]) => {
            const list = tasks.filter((t) => t.column === column);
            return (
              <section key={column} className="tcol" aria-label={label}>
                <div className="ch">
                  {label}
                  <span>{list.length}</span>
                </div>
                {list.map((t) => (
                  <TaskCard
                    key={t.id}
                    task={t}
                    repository={state.repositories.length > 1 ? repoName(t.repositoryId) : undefined}
                    sessions={sessionsOfTask(state, t.id)}
                    linkingSession={linkingSession}
                    {...props}
                  />
                ))}
              </section>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function TaskCard(props: {
  task: Task;
  repository?: string;
  sessions: SessionView[];
  linkingSession?: SessionView;
  linking: Linking | null;
  send: (command: Command) => void;
  onAssign: (task: Task) => void;
  onOpenSession: (id: string) => void;
}) {
  const { task: t, repository, sessions, linkingSession, linking, send, onAssign, onOpenSession } = props;
  const done = t.checklist.filter((c) => c.done).length;
  const session = sessions[sessions.length - 1];
  return (
    <article className="tc" aria-label={`Task ${t.key}`}>
      <div className="id">
        <span className="src">{SOURCE_LABEL[t.source]}</span>
        <span>{t.key}</span>
        {repository && <span>· {repository}</span>}
        {t.checklist.length > 0 && <span className="chk">{`${done}/${t.checklist.length}`}</span>}
      </div>
      <div className="tt">{t.title}</div>
      <div className="ag">
        {session ? (
          <>
            <Status status={session.status} variant="dot" />
            <span>{AGENT_SHORT[session.agent]}</span>
            <code>{session.branch ?? session.worktreePath}</code>
            <button className="btn sm ghost" onClick={() => onOpenSession(session.id)}>
              Open
            </button>
          </>
        ) : t.column === "done" ? (
          <span className="muted">merged</span>
        ) : (
          !linkingSession && (
            <button className="btn sm primary" onClick={() => onAssign(t)}>
              Assign to agent
            </button>
          )
        )}
        {linkingSession && linking && t.column !== "done" && (
          <button
            className="btn sm primary"
            onClick={() => send({ type: "linkTask", sessionId: linking.sessionId, taskId: t.id, relink: linking.relink })}
          >
            Link to {linkingSession.branch ?? linkingSession.worktreePath}
          </button>
        )}
      </div>
    </article>
  );
}

const KINDS: [TaskKind, string][] = [
  ["feat", "Feature (feat/)"],
  ["fix", "Fix (fix/)"],
  ["chore", "Chore (chore/)"],
];

/** The New local task modal: title, kind, description and checklist (one item per line). */
export function NewTask(props: { state: State; send: (command: Command) => void; onClose: () => void }) {
  const { state, send, onClose } = props;
  const [repositoryId, setRepositoryId] = useState(state.repositories[0]?.id ?? "");
  const [title, setTitle] = useState("");
  const [kind, setKind] = useState<TaskKind>("feat");
  const [description, setDescription] = useState("");
  const [checklist, setChecklist] = useState("");
  const canAdd = !!repositoryId && !!title.trim();

  return (
    <div className="overlay-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <form
        className="sheet"
        role="dialog"
        aria-label="New local task"
        aria-modal="true"
        onSubmit={(e) => {
          e.preventDefault();
          if (!canAdd) return;
          send({
            type: "createTask",
            repositoryId,
            title: title.trim(),
            description: description.trim(),
            kind,
            checklist: checklist
              .split("\n")
              .map((l) => l.trim())
              .filter(Boolean),
          });
          onClose();
        }}
      >
        <div className="modal-h">
          <h2>New local task</h2>
          <button type="button" className="iconbtn" aria-label="Close" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-b">
          <span className="hint">Kept by Switchyard for this repository; it never leaves this machine.</span>
          <div className="row2">
            <label className="field">
              <span>Repository</span>
              <select aria-label="Repository" value={repositoryId} onChange={(e) => setRepositoryId(e.target.value)}>
                {state.repositories.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>Kind</span>
              <select aria-label="Kind" value={kind} onChange={(e) => setKind(e.target.value as TaskKind)}>
                {KINDS.map(([k, label]) => (
                  <option key={k} value={k}>
                    {label}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <label className="field">
            <span>Title</span>
            <input aria-label="Title" autoFocus value={title} onChange={(e) => setTitle(e.target.value)} />
          </label>
          <label className="field">
            <span>Description</span>
            <textarea aria-label="Description" rows={4} value={description} onChange={(e) => setDescription(e.target.value)} />
          </label>
          <label className="field">
            <span>
              Checklist <span className="hint">one step per line; ticks stay local</span>
            </span>
            <textarea aria-label="Checklist" rows={3} value={checklist} onChange={(e) => setChecklist(e.target.value)} />
          </label>
        </div>
        <div className="modal-f">
          <button type="button" className="btn ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn primary" disabled={!canAdd}>
            Add task
          </button>
        </div>
      </form>
    </div>
  );
}

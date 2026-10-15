// The Worktrees view: one card per worktree with its path, base, ahead/behind, dirty count
// and occupying session (free ones dashed), plus the New worktree modal.
import { useEffect, useState } from "react";
import { useAction } from "./actions";
import type { NewSessionPreset } from "./NewSession";
import type { Command } from "./schema/Command";
import type { Worktree } from "./schema/Worktree";
import { sessionOn, type RepositoryView, type State } from "./state";
import { AGENT_LABEL, STATUS_LABEL, Status } from "./status";

export function WorktreesView(props: {
  state: State;
  send: (command: Command) => void;
  onOpenSession: (id: string) => void;
  onNewSession: (preset: NewSessionPreset) => void;
  onNewWorktree: () => void;
  /** Opens a shell tab in the dock on a worktree. */
  onOpenShell: (worktreePath: string) => void;
}) {
  const { state, send, onNewWorktree } = props;
  // Git is the source of truth: re-read facts whenever the view opens.
  useEffect(() => send({ type: "refreshWorktrees" }), [send]);

  return (
    <div className="page">
      <div className="pagehead">
        <h1>Worktrees</h1>
        <span className="sub">Each agent gets its own checkout and branch. Nothing shares a working directory.</span>
        <div className="right">
          <button className="btn primary" onClick={onNewWorktree} disabled={state.repositories.length === 0}>
            New worktree
          </button>
        </div>
      </div>
      {state.repositories.length === 0 && <p className="empty">No repositories yet. Add one with + Repo.</p>}
      {state.repositories.map((r) => (
        <section key={r.id} className="wt-repo" aria-label={`${r.name} worktrees`}>
          <div className="sec">
            <h2>{r.name}</h2>
            <span className="meta">
              {r.worktrees.length} worktree{r.worktrees.length === 1 ? "" : "s"}
            </span>
          </div>
          <div className="wtg">
            {r.worktrees.map((w) => (
              <WorktreeCard key={w.path} repository={r} worktree={w} {...props} />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

function WorktreeCard(props: {
  repository: RepositoryView;
  worktree: Worktree;
  state: State;
  send: (command: Command) => void;
  onOpenSession: (id: string) => void;
  onNewSession: (preset: NewSessionPreset) => void;
  onOpenShell: (worktreePath: string) => void;
}) {
  const { repository, worktree: w, state, send, onOpenSession, onNewSession, onOpenShell } = props;
  const [confirming, setConfirming] = useState(false);
  useAction("confirm.cancel", () => setConfirming(false), confirming);
  const occupant = sessionOn(state, w.path);
  const facts = state.worktreeFacts[w.path];
  const branch = w.branch ?? "(detached)";

  return (
    <article className={`wtc ${occupant ? "" : "free"}`} aria-label={`Worktree ${branch}`}>
      <div className="h">
        <span className={`dot ${occupant ? `st-${occupant.status}` : ""}`} />
        <span className="n">{branch}</span>
        {w.primary && <span className="tag">primary</span>}
        {occupant ? <Status status={occupant.status} /> : <span className="free-tag">free</span>}
      </div>
      <div className="path" title={w.path}>
        {w.path}
      </div>
      <div className="row">
        <span>
          base <b>{repository.baseBranch}</b>
        </span>
        {facts && (
          <>
            <span>
              ↑{facts.ahead} ↓{facts.behind}
            </span>
            <span>dirty {facts.dirty}</span>
            {facts.behind > 0 && <span className="behind">behind base</span>}
            {facts.conflicts.length > 0 && (
              <span className="conflict" title={facts.conflicts.join("\n")}>
                conflicts with {repository.baseBranch}
              </span>
            )}
          </>
        )}
      </div>
      {occupant && <div className="title">{AGENT_LABEL[occupant.agent]}</div>}
      {confirming ? (
        <div className="act confirm" role="group" aria-label={`Remove ${branch}?`}>
          <span>
            Remove {branch}? The branch is kept.
            {occupant && ` Its ${STATUS_LABEL[occupant.status]} session is archived.`}
          </span>
          <button
            className="btn sm danger"
            onClick={() => {
              setConfirming(false);
              send({ type: "removeWorktree", worktreePath: w.path });
            }}
          >
            Remove worktree
          </button>
          <button className="btn sm ghost" onClick={() => setConfirming(false)}>
            Cancel
          </button>
        </div>
      ) : (
        <div className="act">
          {occupant ? (
            <button className="btn sm" onClick={() => onOpenSession(occupant.id)}>
              Open session
            </button>
          ) : w.primary ? (
            <>
              <button className="btn sm" onClick={() => onNewSession({ worktreePath: w.path, mode: "explore" })}>
                Explore here
              </button>
              <button
                className="btn sm"
                title="Fixes go through a fix/ branch and a pull request, never straight onto the primary worktree"
                onClick={() => onNewSession({ repositoryId: repository.id, branchKind: "fix" })}
              >
                Quick fix
              </button>
            </>
          ) : (
            <button className="btn sm primary" onClick={() => onNewSession({ worktreePath: w.path })}>
              Start agent here
            </button>
          )}
          {!w.primary && !!facts?.behind && (
            <button className="btn sm" onClick={() => send({ type: "rebaseWorktree", worktreePath: w.path })}>
              Rebase on {repository.baseBranch}
            </button>
          )}
          <button className="btn sm ghost" title="Open a shell here in the dock" onClick={() => onOpenShell(w.path)}>
            Terminal
          </button>
          {!w.primary && (
            <button
              className="btn sm ghost"
              title={facts?.dirty ? "Has uncommitted changes" : undefined}
              onClick={() => setConfirming(true)}
            >
              Remove
            </button>
          )}
        </div>
      )}
    </article>
  );
}

export function NewWorktree(props: { state: State; send: (command: Command) => void; onClose: () => void }) {
  const { state, send, onClose } = props;
  const [repositoryId, setRepositoryId] = useState(state.repositories[0]?.id ?? "");
  const [branch, setBranch] = useState("");
  const repository = state.repositories.find((r) => r.id === repositoryId);
  const canCreate = !!repository && !!branch.trim();

  return (
    <div className="overlay-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <form
        className="sheet"
        role="dialog"
        aria-label="New worktree"
        aria-modal="true"
        onSubmit={(e) => {
          e.preventDefault();
          if (!canCreate) return;
          send({ type: "createWorktree", repositoryId, branch: branch.trim() });
          onClose();
        }}
      >
        <div className="modal-h">
          <h2>New worktree</h2>
          <button type="button" className="iconbtn" aria-label="Close" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-b">
          <span className="hint">
            Runs <code>git worktree add</code> with a new branch, so an agent can work without touching your main
            checkout.
          </span>
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
            <span>
              Branch <span className="hint">from {repository?.baseBranch ?? "base"}</span>
            </span>
            <input
              aria-label="Branch"
              className="mono"
              autoFocus
              value={branch}
              placeholder="feat/my-change"
              onChange={(e) => setBranch(e.target.value)}
            />
          </label>
        </div>
        <div className="modal-f">
          <button type="button" className="btn ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn primary" disabled={!canCreate}>
            Create
          </button>
        </div>
      </form>
    </div>
  );
}

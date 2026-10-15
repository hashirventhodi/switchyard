// Where a worktree's branch stands against its base: the ↓N badge, the Rebase action, and
// the review pane's History and Context tabs (commits above the base marker; branch facts).
import { useState } from "react";
import { usePaletteSource } from "./actions";
import { ChangesTab } from "./ChangesTab";
import type { ChangeKind } from "./schema/ChangeKind";
import type { Command } from "./schema/Command";
import { useWorktreeChanged } from "./useWorktreeChanged";
import type { WorktreeFacts } from "./schema/WorktreeFacts";
import type { SessionView, State } from "./state";
import { ContextArchive } from "./ContextArchive";
import { ContextGrants } from "./ContextGrants";
import { ContextDeclaredMcp } from "./ContextDeclaredMcp";
import { ContextFiles } from "./ContextFiles";
import { ContextMcp } from "./ContextMcp";
import { ContextTask } from "./ContextTask";
import { HistoryPullRequest } from "./HistoryPullRequest";
import type { OpenLink } from "./Setup";
import { ContextSecrets } from "./ContextSecrets";

/** "↓3" (warn colour) when the worktree is behind its base; nothing otherwise. */
export function BehindBadge({ facts, base }: { facts?: WorktreeFacts; base?: string }) {
  if (!facts?.behind) return null;
  return (
    <span className="behind-n" title={`${facts.behind} commit${facts.behind === 1 ? "" : "s"} on ${base ?? "base"} not in this branch`}>
      ↓{facts.behind}
      {base && <> behind {base}</>}
    </span>
  );
}

/** The repository base and facts for a worktree path, if known. */
export function branchOf(state: State, worktreePath: string) {
  const repository = state.repositories.find((r) => r.worktrees.some((w) => w.path === worktreePath));
  const worktree = repository?.worktrees.find((w) => w.path === worktreePath);
  return { repository, worktree, facts: state.worktreeFacts[worktreePath] };
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/**
 * The review pane beside the agent pane. History and Context here; the Changes tab joins
 * them (#19).
 */
export function BranchPane(props: {
  state: State;
  session: SessionView;
  send: (command: Command) => void;
  /** The pane has the full width (Expand). */
  expanded?: boolean;
  onToggleExpand?: () => void;
  /** Put text in the composer for the user to finish and send (Ask agent). */
  onAsk?: (text: string) => void;
  /** Open Tasks to pick the session's task ("Pick one"; `relink` to change it). */
  onPickTask?: (sessionId: string, relink: boolean) => void;
  /** Open an https link in the browser (pull request, checks). */
  openLink?: OpenLink;
}) {
  const { state, session, send, expanded = false, onToggleExpand, onAsk = () => {} } = props;
  const [tab, setTab] = useState<"changes" | "history" | "context">("changes");
  const path = session.worktreePath;
  const { repository, facts } = branchOf(state, path);
  // Badges read the pushed facts only: right before either tab is opened, and while another is.
  const count = facts?.commits;
  const changed = facts?.changedFiles;
  usePaletteSource(() =>
    onToggleExpand
      ? [{ id: "review.expand", group: "Actions", title: expanded ? "Show agent beside review" : "Expand review pane", run: onToggleExpand }]
      : [],
  );
  const tabButton = (id: typeof tab, label: string, n?: number) => (
    <button role="tab" className={`tab ${tab === id ? "active" : ""}`} aria-selected={tab === id} onClick={() => setTab(id)}>
      {label} {n !== undefined && <span className="n">{n}</span>}
    </button>
  );
  return (
    <aside className="pane changes-pane" aria-label="Review">
      <div className="pane-head">
        <div className="tabs" role="tablist">
          {tabButton("changes", "Changes", changed)}
          {tabButton("history", "History", count)}
          {tabButton("context", "Context")}
        </div>
        <div className="spacer" />
        {onToggleExpand && (
          <button className="btn ghost sm" title="Expand review pane" aria-pressed={expanded} onClick={onToggleExpand}>
            {expanded ? "Show agent" : "Expand"}
          </button>
        )}
      </div>
      {tab === "changes" ? (
        <ChangesTab
          session={session}
          files={state.changes[path]}
          facts={facts}
          base={repository?.baseBranch}
          send={send}
          onAsk={onAsk}
          conflicts={state.conflicts[path]?.files}
          worktreeChanges={state.worktreeChanges}
        />
      ) : (
        <div className="pane-body" role="tabpanel">
          {tab === "history" ? (
            <>
              <HistoryTab {...props} />
              <HistoryPullRequest state={state} worktreePath={path} openLink={props.openLink ?? (() => {})} />
            </>
          ) : (
            <>
              <ContextTask {...props} />
              <BranchContext {...props} />
              <ContextGrants {...props} />
              <ContextMcp {...props} />
              <ContextDeclaredMcp {...props} />
              <ContextFiles {...props} />
              <ContextSecrets {...props} />
              <ContextArchive {...props} />
            </>
          )}
        </div>
      )}
    </aside>
  );
}

const HISTORY_KINDS: ChangeKind[] = ["git"];

/** The branch's commits, newest first, above the base marker (only the marker when none). */
export function HistoryTab({ state, session, send }: { state: State; session: SessionView; send: (command: Command) => void }) {
  const path = session.worktreePath;
  const { repository, facts } = branchOf(state, path);
  const history = state.history[path];
  // Read on open, and again whenever git's records move: a commit, an amend, a rebase.
  useWorktreeChanged(state.worktreeChanges, path, HISTORY_KINDS, () => send({ type: "readHistory", worktreePath: path }));
  const base = history?.base ?? repository?.baseBranch ?? "base";
  const behind = facts?.behind ?? 0;
  const canAsk = session.status === "waiting" || session.status === "done" || session.status === "failed";
  // Rebase is mechanical: never under an agent mid-conversation (Running, Needs you).
  const canRebase = session.status === "idle" || session.status === "done" || session.status === "failed";
  return (
    <ul className="graph" aria-label={`Commits on ${session.branch ?? path}`}>
      {history?.commits.map((c) => (
        <li className="c" key={c.hash} title={`${c.hash}\n${c.author}`}>
          <span className="n" />
          <span className="h">{c.short}</span>
          <span className="m">{c.subject}</span>
          <span className="t">{c.author}</span>
        </li>
      ))}
      <li className="c">
        <span className="n base" />
        <span className="h">{base}</span>
        <span className="m muted">
          base · {behind ? `${plural(behind, "commit")} ahead of this branch` : "up to date"}
          {!!facts?.conflicts.length && <span className="bad"> · conflicts in {facts.conflicts.join(", ")}</span>}
        </span>
        <span className="t">
          {behind > 0 && (
            <>
              <button
                className="btn sm"
                disabled={!canRebase}
                title={canRebase ? `Rebase on ${base}; a conflict stops it for you to resolve` : "Rebase when the session is Paused, Finished or Error, or ask the agent"}
                onClick={() => send({ type: "rebaseWorktree", worktreePath: path })}
              >
                Rebase
              </button>
              <button
                className="btn sm"
                disabled={!canAsk}
                title={canAsk ? "Send the agent one message asking it to rebase and resolve conflicts" : "The agent is busy or paused"}
                onClick={() => send({ type: "askAgentToRebase", sessionId: session.id })}
              >
                Ask agent to rebase
              </button>
            </>
          )}
        </span>
      </li>
    </ul>
  );
}

/** Context's Worktree section: repository and base, branch ↑/↓ with rebase, path and dirty count. */
export function BranchContext({ state, session, send }: { state: State; session: SessionView; send: (command: Command) => void }) {
  const path = session.worktreePath;
  const { repository, facts } = branchOf(state, path);
  const branch = session.branch ?? "(detached)";
  const base = repository?.baseBranch ?? "base";
  return (
    <div className="ctx">
      <h4>Worktree</h4>
      <div className="kv">
        <span className="k">{repository?.name}</span>
        <span className="v">{base}</span>
      </div>
      <div className="kv" role="group" aria-label={`Branch ${branch}`}>
        <span className="k">{branch}</span>
        <span className={`v ${facts?.behind ? "warn" : ""}`}>
          {facts ? (
            <>
              {facts.ahead > 0 && `↑${facts.ahead} `}
              {facts.behind > 0 && `↓${facts.behind} `}
              {!facts.ahead && !facts.behind && "up to date"}
              {facts.behind > 0 && (
                <button
                  aria-label={`Rebase on ${base}`}
                  disabled={session.status === "running" || session.status === "waiting"}
                  onClick={() => send({ type: "rebaseWorktree", worktreePath: path })}
                >
                  rebase
                </button>
              )}
            </>
          ) : (
            "—"
          )}
        </span>
      </div>
      <div className="kv">
        <span className="k" title={path}>
          {path}
        </span>
        <span className="v">{facts ? `${facts.dirty} changed` : ""}</span>
      </div>
    </div>
  );
}

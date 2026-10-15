// Where a worktree's branch stands against its base: the ↓N badge, the Rebase action, and
// the review pane's History and Context tabs (commits above the base marker; branch facts).
import { useEffect, useState } from "react";
import type { Command } from "./schema/Command";
import type { WorktreeFacts } from "./schema/WorktreeFacts";
import type { SessionView, State } from "./state";
import { ContextGrants } from "./ContextGrants";

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
export function BranchPane(props: { state: State; session: SessionView; send: (command: Command) => void }) {
  const [tab, setTab] = useState<"history" | "context">("history");
  const { history } = props.state;
  const count = history[props.session.worktreePath]?.commits.length;
  return (
    <aside className="pane changes-pane" aria-label="Review">
      <div className="pane-head"><div className="tabs" role="tablist">
        <button role="tab" className={`tab ${tab === "history" ? "active" : ""}`} aria-selected={tab === "history"} onClick={() => setTab("history")}>
          History {count !== undefined && <span className="n">{count}</span>}
        </button>
        <button role="tab" className={`tab ${tab === "context" ? "active" : ""}`} aria-selected={tab === "context"} onClick={() => setTab("context")}>
          Context
        </button>
        </div>
      </div>
      <div className="pane-body" role="tabpanel">
        {tab === "history" ? (
          <HistoryTab {...props} />
        ) : (
          <>
            <BranchContext {...props} />
            <ContextGrants {...props} />
          </>
        )}
      </div>
    </aside>
  );
}

/** The branch's commits, newest first, above the base marker (only the marker when none). */
export function HistoryTab({ state, session, send }: { state: State; session: SessionView; send: (command: Command) => void }) {
  const path = session.worktreePath;
  const { repository, facts } = branchOf(state, path);
  const history = state.history[path];
  // Re-read whenever the branch moves: a commit, a rebase, base advancing.
  useEffect(
    () => send({ type: "readHistory", worktreePath: path }),
    [send, path, facts?.ahead, facts?.behind],
  );
  const base = history?.base ?? repository?.baseBranch ?? "base";
  const behind = facts?.behind ?? 0;
  const canAsk = session.status === "waiting" || session.status === "done" || session.status === "failed";
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
              <button className="btn sm" onClick={() => send({ type: "rebaseWorktree", worktreePath: path })}>
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
                <button aria-label={`Rebase on ${base}`} onClick={() => send({ type: "rebaseWorktree", worktreePath: path })}>
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

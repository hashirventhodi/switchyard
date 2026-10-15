// Shipping a worktree's branch (ADR-0003): the pull request substate shown under a status
// ("PR #421 open · checks running"), the Open pull request / View PR / Local merge actions,
// and gh's setup state for the Dashboard. The core pushes, opens, polls and cleans up; this
// only sends commands and shows what it reports.
import type { Command } from "./schema/Command";
import type { ForgeSetup } from "./schema/ForgeSetup";
import type { PullRequest } from "./schema/PullRequest";
import type { OpenLink } from "./Setup";
import { branchOf } from "./History";
import type { State } from "./state";

const CHECKS: Record<PullRequest["checks"], string | null> = {
  none: null,
  running: "checks running",
  failing: "checks failing",
  passing: "checks passing",
};
const REVIEW: Record<PullRequest["review"], string | null> = {
  none: null,
  required: "review required",
  changesRequested: "changes requested",
  approved: "approved",
};

/** "PR #421 open · checks running · approved", "PR #421 merged", "PR #421 closed". */
export function prSubstate(pr: PullRequest): string {
  if (pr.state !== "open") return `PR #${pr.number} ${pr.state}`;
  const parts = [`PR #${pr.number} ${pr.draft ? "draft" : "open"}`, CHECKS[pr.checks], REVIEW[pr.review]];
  return parts.filter(Boolean).join(" · ");
}

function tone(pr: PullRequest): string {
  if (pr.state === "merged") return "ok";
  if (pr.state === "closed") return "muted";
  if (pr.checks === "failing" || pr.review === "changesRequested") return "bad";
  if (pr.review === "approved" && pr.checks !== "running") return "ok";
  return "";
}

/** The pull request substate for a worktree; nothing when it has none. */
export function PrBadge({ state, worktreePath }: { state: State; worktreePath: string }) {
  const pr = state.pullRequests[worktreePath];
  if (!pr) return null;
  return (
    <span className={`pr-sub ${tone(pr)}`} title={pr.url}>
      {prSubstate(pr)}
    </span>
  );
}

/**
 * How work leaves this worktree: Open pull request (or View PR #N while one is open) when
 * its repository has a remote, Local merge into base when it has none. Nothing on the
 * primary worktree.
 */
export function ShipActions(props: {
  state: State;
  worktreePath: string;
  send: (command: Command) => void;
  openLink: OpenLink;
  primary?: boolean;
}) {
  const { state, worktreePath, send, openLink } = props;
  const { repository, worktree } = branchOf(state, worktreePath);
  if (!repository || worktree?.primary) return null;
  if (!repository.remote) {
    return (
      <button
        className="btn sm"
        title="This repository has no remote: merge into base in the primary worktree"
        onClick={() => send({ type: "localMerge", worktreePath })}
      >
        Local merge into {repository.baseBranch}
      </button>
    );
  }
  const pr = state.pullRequests[worktreePath];
  if (pr?.state === "open") {
    return (
      <button className="btn sm" title={pr.url} onClick={() => openLink(pr.url)}>
        View PR #{pr.number}
      </button>
    );
  }
  return (
    <button
      className={`btn sm ${props.primary ? "primary" : ""}`}
      title={`Push the branch and open a pull request against ${repository.baseBranch}`}
      onClick={() => send({ type: "openPullRequest", worktreePath })}
    >
      Open pull request
    </button>
  );
}

/** Prune merged for one repository (only when it has a remote). */
export function PruneMerged(props: { state: State; repositoryId: string; send: (command: Command) => void }) {
  const repository = props.state.repositories.find((r) => r.id === props.repositoryId);
  if (!repository?.remote) return null;
  return (
    <button
      className="btn sm ghost"
      title="Remove every free, clean worktree whose pull request is merged"
      onClick={() => props.send({ type: "pruneMerged", repositoryId: props.repositoryId })}
    >
      Prune merged
    </button>
  );
}

/** gh's problem on the Dashboard's setup state; nothing when it is ready. */
export function ForgeProblem({ forge, openLink }: { forge: ForgeSetup; openLink: OpenLink }) {
  const { state } = forge;
  if (state.type === "ready") return null;
  return (
    <div className="setup-item">
      {state.type === "missing" ? (
        <>
          <strong>GitHub CLI (gh) is not installed</strong>
          <span className="setup-hint">
            Install it with: <code>{state.installHint}</code>
          </span>
          <button type="button" className="btn ghost sm" title={state.link.url} onClick={() => openLink(state.link.url)}>
            {state.link.label}
          </button>
        </>
      ) : (
        <>
          <strong>GitHub CLI (gh) is not signed in</strong>
          <span className="setup-hint">
            Run <code>{state.loginHint}</code> in a terminal, then check again.
          </span>
        </>
      )}
    </div>
  );
}

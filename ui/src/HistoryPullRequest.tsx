// History's pull request: number, state and every check the Forge reports, beneath the
// commits. Read from `state.pullRequests` (the core polls the Forge); nothing when the
// branch has no pull request.
import { CheckedAgo } from "./CheckedAgo";
import type { CheckStatus } from "./schema/CheckStatus";
import type { OpenLink } from "./Setup";
import { prSubstate } from "./PullRequest";
import type { State } from "./state";

const STATUS_LABEL: Record<CheckStatus, string> = {
  pending: "pending",
  running: "running",
  passing: "passing",
  failing: "failing",
  skipped: "skipped",
};
const STATUS_MARK: Record<CheckStatus, string> = {
  pending: "○",
  running: "◐",
  passing: "✓",
  failing: "✕",
  skipped: "–",
};

export function HistoryPullRequest({ state, worktreePath, openLink }: { state: State; worktreePath: string; openLink: OpenLink }) {
  const pr = state.pullRequests[worktreePath];
  if (!pr) return null;
  const checked = state.pullRequestChecked[worktreePath];
  return (
    <section className="ctx history-pr" aria-label="Pull request">
      <h4>
        <span>Pull request</span>
        {checked !== undefined && <CheckedAgo at={checked} />}
      </h4>
      <div className="kv">
        <span className="k">
          #{pr.number} <span className={`pr-state ${pr.state}`}>{pr.draft && pr.state === "open" ? "draft" : pr.state}</span>
        </span>
        <span className="v">
          <button title={pr.url} onClick={() => openLink(pr.url)}>
            View PR #{pr.number}
          </button>
        </span>
      </div>
      <p className="pr-summary">{prSubstate(pr)}</p>
      {pr.checkRuns.length ? (
        <ul className="checks" aria-label="Checks">
          {pr.checkRuns.map((c, i) => (
            <li key={`${i}-${c.name}`} className={`check-run ${c.status}`}>
              <span className="mark" aria-hidden>
                {STATUS_MARK[c.status]}
              </span>
              {c.url ? (
                <button className="name" title={c.url} onClick={() => openLink(c.url!)}>
                  {c.name}
                </button>
              ) : (
                <span className="name">{c.name}</span>
              )}
              <span className="st">{STATUS_LABEL[c.status]}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="empty">No checks reported</p>
      )}
    </section>
  );
}

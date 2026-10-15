// Context's Policy sections: the repository's default mode and ceiling, and every Grant and
// denial that applies to it with its layer (repo file or local), Revoke on local ones.
import { useEffect } from "react";
import { describeRule, LAYER_LABEL } from "./grants";
import type { Command } from "./schema/Command";
import type { PolicyRequest } from "./schema/PolicyRequest";
import type { SessionView, State } from "./state";
import { MODE_LABEL } from "./status";

export function ContextGrants({ state, session, send }: { state: State; session: SessionView; send: (command: Command) => void }) {
  const repositoryId = session.repositoryId;
  // The policy file lives in git and may have changed: read it whenever Context opens.
  useEffect(() => send({ type: "readPolicy", repositoryId }), [repositoryId, send]);
  const policy = state.policies[repositoryId];
  return (
    <>
      {policy?.request && (
        <RequestBox request={policy.request} onAcknowledge={() => send({ type: "acknowledgePolicy", repositoryId, hash: policy.request!.hash })} />
      )}
      <section className="ctx grants" aria-label="Grants">
        <h4>Policy</h4>
        {!policy ? (
          <p className="empty">Reading policy…</p>
        ) : (
          <>
            {policy.error && (
              <p className="policy-error" role="alert">
                {policy.error}. Nothing is granted until it is fixed.
              </p>
            )}
            <div className="kv">
              <span className="k">Default mode</span>{" "}
              <span className="v">{policy.defaultMode ? MODE_LABEL[policy.defaultMode] : "—"}</span>
            </div>
            <div className="kv">
              <span className="k">Ceiling</span> <span className="v">{policy.ceiling ? MODE_LABEL[policy.ceiling] : "none"}</span>
            </div>
            <h4>Grants</h4>
            {policy.grants.length === 0 ? (
              <p className="empty">Nothing runs without asking. Answer a request with Always allow to add one.</p>
            ) : (
              <ul className="grant-list">
                {policy.grants.map((grant) => {
                  const rule = describeRule(grant.rule);
                  return (
                    <li key={grant.id} className={`grant ${grant.effect}`}>
                      <span className="eff">{grant.effect === "deny" ? "Deny" : "Allow"}</span>
                      <code>{rule}</code>
                      <span className="layer">{LAYER_LABEL[grant.layer]}</span>
                      {grant.layer === "local" && (
                        <button
                          aria-label={`Revoke ${rule}`}
                          onClick={() => send({ type: "revokeGrant", repositoryId, grantId: grant.id })}
                        >
                          Revoke
                        </button>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
          </>
        )}
      </section>
    </>
  );
}

/** What the committed policy asks for beyond what applies (ADR-0006), with Acknowledge. */
function RequestBox({ request, onAcknowledge }: { request: PolicyRequest; onAcknowledge: () => void }) {
  return (
    <section className="ctx policy-request" aria-label="Policy request">
      <h4>Repository policy requests</h4>
      <p className="note">Committed to the repository; none of this applies until you acknowledge it. Any change asks again.</p>
      <ul>
        {request.allow.map((g) => (
          <li key={g.id}>
            Allow <code>{describeRule(g.rule)}</code>
          </li>
        ))}
        {request.sandbox && (
          <li>
            A sandbox: Full auto would skip the CLI&apos;s own checks.{" "}
            <span className="evidence">
              {request.sandboxEvidence ?? "Switchyard cannot verify this sandbox: it detects no container or VM here."}
            </span>
          </li>
        )}
        {request.defaultMode && <li>Default mode {MODE_LABEL[request.defaultMode]}</li>}
        {request.worktreesRoot && (
          <li>
            Worktrees under <code>{request.worktreesRoot}</code>
          </li>
        )}
      </ul>
      <button className="btn sm primary" onClick={onAcknowledge}>
        Acknowledge
      </button>
    </section>
  );
}

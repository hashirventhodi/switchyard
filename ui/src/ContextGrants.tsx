// Context's Policy sections: the repository's default mode and ceiling, and every Grant and
// denial that applies to it with its layer (repo file or local), Revoke on local ones.
import { useEffect } from "react";
import { describeRule, LAYER_LABEL } from "./grants";
import type { Command } from "./schema/Command";
import type { SessionView, State } from "./state";
import { MODE_LABEL } from "./status";

export function ContextGrants({ state, session, send }: { state: State; session: SessionView; send: (command: Command) => void }) {
  const repositoryId = session.repositoryId;
  // The policy file lives in git and may have changed: read it whenever Context opens.
  useEffect(() => send({ type: "readPolicy", repositoryId }), [repositoryId, send]);
  const policy = state.policies[repositoryId];
  return (
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
  );
}

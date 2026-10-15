// Context's Secrets section: the repository's secrets by name (declared in its policy file,
// referenced by its .mcp.json, or added here) with Grant / Revoke, and where each value
// would come from. A value can be kept in the OS keychain from here; it is never shown.
import { useEffect, useState } from "react";
import { useAction } from "./actions";
import type { Command } from "./schema/Command";
import type { SecretOrigin } from "./schema/SecretOrigin";
import type { SecretSource } from "./schema/SecretSource";
import type { SessionView, State } from "./state";

const ORIGIN_LABEL: Record<SecretOrigin, string> = {
  policy: "policy file",
  mcp: ".mcp.json",
  local: "added here",
};

const SOURCE_LABEL: Record<SecretSource, string> = {
  keychain: "keychain",
  environment: "environment",
  missing: "no value",
};

export function ContextSecrets({ state, session, send }: { state: State; session: SessionView; send: (command: Command) => void }) {
  const repositoryId = session.repositoryId;
  // The policy file and .mcp.json live in git and may have changed: read whenever Context opens.
  useEffect(() => send({ type: "readSecrets", repositoryId }), [repositoryId, send]);
  const secrets = state.secrets[repositoryId];
  const [name, setName] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [value, setValue] = useState("");
  const closeEditor = () => {
    setEditing(null);
    setValue("");
  };
  useAction("confirm.cancel", closeEditor, editing !== null);

  const add = () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    send({ type: "addSecret", repositoryId, name: trimmed });
    setName("");
  };
  const save = (secret: string) => {
    if (!value) return;
    send({ type: "setSecretValue", repositoryId, name: secret, value });
    closeEditor();
  };
  const granted = secrets?.filter((s) => s.granted).length ?? 0;

  return (
    <section className="ctx secrets" aria-label="Secrets">
      <h4>
        Secrets{secrets && secrets.length > 0 && <span>{`${granted}/${secrets.length} granted`}</span>}
      </h4>
      {!secrets ? (
        <p className="empty">Reading secrets…</p>
      ) : secrets.length === 0 ? (
        <p className="empty">No secrets. Name them in .switchyard/policy.yml under secrets, or add one here.</p>
      ) : (
        <ul className="secret-list">
          {secrets.map((secret) => (
            <li key={secret.name} className={`secret ${secret.granted ? "granted" : ""}`}>
              <div className="row">
                <code>{secret.name}</code>
                <small>{ORIGIN_LABEL[secret.origin]}</small>
                <span className={`src ${secret.source}`}>{SOURCE_LABEL[secret.source]}</span>
                {secret.granted && <span className="flag">granted</span>}
                <span className="spacer" />
                <button aria-label={`Set value of ${secret.name}`} onClick={() => setEditing(secret.name)}>
                  Set value
                </button>
                {secret.granted ? (
                  <button aria-label={`Revoke ${secret.name}`} onClick={() => send({ type: "revokeSecret", repositoryId, name: secret.name })}>
                    Revoke
                  </button>
                ) : (
                  <button aria-label={`Grant ${secret.name}`} onClick={() => send({ type: "grantSecret", repositoryId, name: secret.name })}>
                    Grant
                  </button>
                )}
                {secret.origin === "local" && (
                  <button aria-label={`Remove ${secret.name}`} onClick={() => send({ type: "removeSecret", repositoryId, name: secret.name })}>
                    Remove
                  </button>
                )}
              </div>
              {editing === secret.name && (
                <form
                  className="secret-value"
                  onSubmit={(e) => {
                    e.preventDefault();
                    save(secret.name);
                  }}
                >
                  <input
                    type="password"
                    autoFocus
                    autoComplete="off"
                    aria-label={`Value of ${secret.name}`}
                    value={value}
                    onChange={(e) => setValue(e.target.value)}
                  />
                  <button type="submit" className="btn sm" disabled={!value}>
                    Save to keychain
                  </button>
                  <button type="button" className="btn ghost sm" onClick={closeEditor}>
                    Cancel
                  </button>
                </form>
              )}
            </li>
          ))}
        </ul>
      )}
      {secrets && (
        <form
          className="secret-add"
          onSubmit={(e) => {
            e.preventDefault();
            add();
          }}
        >
          <input
            type="text"
            aria-label="Secret name"
            placeholder="SECRET_NAME"
            spellCheck={false}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <button type="submit" className="btn ghost sm" disabled={!name.trim()}>
            Add secret
          </button>
        </form>
      )}
      <p className="hint">Granted secrets reach this repository's agents and their MCP servers from their next start.</p>
    </section>
  );
}

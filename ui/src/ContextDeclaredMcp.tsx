// Context's Declared MCP servers: what the repository's committed .mcp.json lists. Committed
// configuration is a request (ADR-0006): a server reaches the agent only once enabled here,
// for its definition as it stands; a changed definition asks again.
import { useEffect } from "react";
import type { Command } from "./schema/Command";
import type { DeclaredMcpServer } from "./schema/DeclaredMcpServer";
import type { SessionView, State } from "./state";

function stateText(s: DeclaredMcpServer): string {
  if (s.enabled) return "enabled";
  return s.changed ? "changed since you enabled it" : "declared, not enabled";
}

export function ContextDeclaredMcp(props: { state: State; session: SessionView; send: (command: Command) => void }) {
  const { state, session, send } = props;
  const repositoryId = session.repositoryId;
  useEffect(() => send({ type: "readMcpServers", repositoryId }), [repositoryId, send]);
  const declared = state.mcpDeclared[repositoryId];
  return (
    <section className="ctx mcp declared" aria-label="Declared MCP servers">
      <h4>Declared MCP servers</h4>
      {declared?.error && <p className="note bad">{declared.error}</p>}
      {!declared ? (
        <p className="empty">Reading .mcp.json…</p>
      ) : declared.servers.length === 0 ? (
        <p className="empty">The repository's .mcp.json declares no servers.</p>
      ) : (
        <>
          <ul className="mcp-list">
            {declared.servers.map((s) => (
              <li key={s.name} className="kv">
                <span className="k">
                  {s.name}
                  <small>{s.transport}</small>
                </span>
                <span className={`v ${s.enabled ? "ok" : "no"}`}>{stateText(s)}</span>
                <button
                  className="btn sm ghost"
                  aria-label={`${s.enabled ? "Disable" : "Enable"} ${s.name}`}
                  onClick={() =>
                    send({ type: s.enabled ? "disableMcpServer" : "enableMcpServer", repositoryId, name: s.name })
                  }
                >
                  {s.enabled ? "Disable" : "Enable"}
                </button>
              </li>
            ))}
          </ul>
          <p className="note">Committed by the repository; nothing runs until you enable it. Changes apply from the next session.</p>
        </>
      )}
    </section>
  );
}

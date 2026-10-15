// Context's MCP servers section: each server the session's CLI was given (the repository's
// .mcp.json, passed per spawn) with its connection state as the CLI last reported it.
import type { McpState } from "./schema/McpState";
import type { SessionView } from "./state";

const STATE_LABEL: Record<McpState, string> = {
  connected: "connected",
  pending: "connecting…",
  failed: "not connected",
  needsAuth: "needs sign-in",
  disabled: "disabled",
};

export function ContextMcp({ session }: { session: SessionView }) {
  const servers = session.facts.mcpServers ?? [];
  const connected = servers.filter((s) => s.state === "connected").length;
  return (
    <section className="ctx mcp" aria-label="MCP servers">
      <h4>
        MCP servers{servers.length > 0 && <span>{`${connected}/${servers.length}`}</span>}
      </h4>
      {servers.length === 0 ? (
        <p className="empty">No MCP servers. Add them to the repository's .mcp.json.</p>
      ) : (
        <ul className="mcp-list">
          {servers.map((server) => (
            <li key={server.name} className="kv">
              <span className="k">
                {server.name}
                <small>{server.transport}</small>
              </span>
              <span className={`v ${server.state === "connected" ? "ok" : "no"}`} title={server.error ?? undefined}>
                {STATE_LABEL[server.state]}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

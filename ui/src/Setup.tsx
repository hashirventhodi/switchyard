// Agent setup: what the core detected about each agent CLI (installed, in the range this build
// supports). The Dashboard lists every agent that is not ready; New session shows why an agent
// cannot start. Links open in the user's browser through `openLink`; nothing here fetches.
import type { AgentKind } from "./schema/AgentKind";
import type { AgentSetup } from "./schema/AgentSetup";
import type { ForgeSetup } from "./schema/ForgeSetup";
import type { Link } from "./schema/Link";
import { ForgeProblem } from "./PullRequest";

/** Opens a link in the user's browser. */
export type OpenLink = (url: string) => void;

/** Why `agent` cannot start a session, or null when it can (or nothing is known yet). */
export function agentProblem(agents: AgentSetup[], agent: AgentKind): AgentSetup | null {
  const found = agents.find((a) => a.agent === agent);
  return found && found.state.type !== "ready" ? found : null;
}

function LinkButton({ link, openLink }: { link: Link; openLink: OpenLink }) {
  return (
    <button type="button" className="btn ghost sm" title={link.url} onClick={() => openLink(link.url)}>
      {link.label}
    </button>
  );
}

/** One agent's problem: the message (or install hint) and its link. */
export function AgentProblem({ setup, openLink }: { setup: AgentSetup; openLink: OpenLink }) {
  const { state, name } = setup;
  if (state.type === "ready") return null;
  return (
    <div className="setup-item">
      {state.type === "missing" ? (
        <>
          <strong>{name} is not installed</strong>
          <span className="setup-hint">
            Install it with: <code>{state.installHint}</code>
          </span>
        </>
      ) : (
        <span>{state.message}</span>
      )}
      {state.link && <LinkButton link={state.link} openLink={openLink} />}
    </div>
  );
}

/** The Dashboard's setup state; renders nothing when every agent and gh are ready. */
export function SetupPanel(props: { agents: AgentSetup[]; forge?: ForgeSetup | null; onCheck: () => void; openLink: OpenLink }) {
  const { agents, forge, onCheck, openLink } = props;
  const problems = agents.filter((a) => a.state.type !== "ready");
  const forgeProblem = forge && forge.state.type !== "ready" ? forge : null;
  if (problems.length === 0 && !forgeProblem) return null;
  return (
    <section className="setup" aria-label="Setup">
      <div className="setup-head">
        <h2>Setup</h2>
        <span className="spacer" />
        <button type="button" className="btn ghost sm" onClick={onCheck}>
          Check again
        </button>
      </div>
      {problems.map((a) => (
        <AgentProblem key={a.agent} setup={a} openLink={openLink} />
      ))}
      {forgeProblem && <ForgeProblem forge={forgeProblem} openLink={openLink} />}
    </section>
  );
}

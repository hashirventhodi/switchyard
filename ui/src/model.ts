// A Session's Model: the choice made at start (a Model id, or Default, which passes nothing
// to the CLI) and how it reads once the Session runs. Default is never shown opaque: it
// reads as what it resolved to, or says the CLI did not report it.
import type { AgentKind } from "./schema/AgentKind";
import type { AgentSetup } from "./schema/AgentSetup";
import type { ModelChoice } from "./schema/ModelChoice";
import type { SessionFacts } from "./schema/SessionFacts";

export const DEFAULT_MODEL: ModelChoice = { type: "default" };

/** Agents whose Model can be chosen at start (Codex's comes with #39). */
const CHOOSES_MODEL: Record<AgentKind, boolean> = { claudeCode: true, codex: false };

export function choosesModel(agent: AgentKind): boolean {
  return CHOOSES_MODEL[agent];
}

/** Whether the agent's adapter switches a running Session's Model live; otherwise a switch
 * applies when the Session resumes. Before the agents are checked it is taken as live (the
 * core decides either way). */
export function switchesModelLive(agents: AgentSetup[], agent: AgentKind): boolean {
  return agents.find((a) => a.agent === agent)?.switchModelLive ?? true;
}

/** What a typed Model id means: blank is Default. */
export function modelChoice(text: string): ModelChoice {
  const id = text.trim();
  return id ? { type: "model", id } : DEFAULT_MODEL;
}

/** The Model a Session runs on, for the summary strip: the CLI's own name for a chosen
 * Model once it reports one, "Default → <resolved>", or "Default (not reported)" once the
 * CLI has started without naming it. */
export function modelLabel(choice: ModelChoice, facts: Pick<SessionFacts, "model" | "cliVersion">): string {
  if (choice.type === "model") return facts.model ?? choice.id;
  if (facts.model) return `Default → ${facts.model}`;
  return facts.cliVersion ? "Default (not reported)" : "Default";
}

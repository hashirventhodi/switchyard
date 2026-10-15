// How Grants read in the renderer, and the pattern "Always allow" offers for a command.
// The core decides what a pattern covers and refuses one that does not cover the request;
// `suggestGrant` mirrors its suggestion (crates/switchyard-core/src/policy/pattern.rs
// `suggest`) so the button can name the pattern before it is sent.
import type { GrantRule } from "./schema/GrantRule";
import type { PolicyLayer } from "./schema/PolicyLayer";
import type { Request } from "./schema/Request";
// The one classifier the sidecar and (in Rust) the core keep in step; pure, no imports.
import { classes } from "../../sidecar/src/dangerous";

const CHAIN = [";", "|", "&", "`", "$(", "<", ">", "\n", "\r"];
const RUNNERS = ["eval", "xargs", "env", "npx"];
const SHELLS = ["bash", "sh", "zsh", "dash"];

const squash = (text: string) => text.split(/\s+/).filter(Boolean).join(" ");
const basename = (word: string) => word.slice(word.lastIndexOf("/") + 1);

function startsWithInterpreter(command: string): boolean {
  const [first = "", flag] = command.split(" ");
  const name = basename(first);
  if (RUNNERS.includes(name)) return true;
  return SHELLS.includes(name) && !!flag && flag.startsWith("-") && !flag.startsWith("--") && flag.includes("c");
}

/** Whether a grant's literal text (before `*`) names every dangerous class of `command`. */
function explicitlyCovers(pattern: string, command: string): boolean {
  const star = pattern.indexOf("*");
  const named = classes(star < 0 ? pattern : pattern.slice(0, star));
  return classes(command).every((c) => named.includes(c));
}

/** The command pattern Always allow grants: the command and its subcommand plus `*`, or the
 * exact command where a wildcard would not cover it (an interpreter, a chain). A dangerous
 * command's pattern grows word by word until it names the command's classes
 * (`npm run db:migrate *`), as only such a grant answers it below Full auto. */
export function suggestCommandPattern(command: string): string {
  const c = squash(command);
  if (startsWithInterpreter(c) || CHAIN.some((ch) => c.includes(ch)) || c.includes("*")) return c;
  const words = c.split(" ");
  const [, sub] = words;
  const start = sub && /^[A-Za-z0-9_:-]+$/.test(sub) && !sub.startsWith("-") ? 2 : 1;
  for (let n = start; n <= words.length; n++) {
    const pattern = `${words.slice(0, n).join(" ")} *`;
    if (explicitlyCovers(pattern, c)) return pattern;
  }
  return c;
}

/** What Always allow would grant for a request; null for kinds never granted. */
export function suggestGrant(request: Request): string | null {
  switch (request.type) {
    case "command":
      return suggestCommandPattern(request.command);
    case "network":
      return request.domain;
    case "mcpTool":
      return `${request.server} · ${request.tool}`;
    case "fileWrite":
    case "question":
      return null;
  }
}

/** How a rule reads: the pattern, the domain, `server · tool`. */
export function describeRule(rule: GrantRule): string {
  switch (rule.type) {
    case "command":
      return rule.pattern;
    case "network":
      return rule.domain;
    case "mcpTool":
      return `${rule.server} · ${rule.tool}`;
  }
}

export const LAYER_LABEL: Record<PolicyLayer, string> = { org: "Organisation", repo: "Repo file", local: "Local" };

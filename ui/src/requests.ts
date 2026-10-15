// How a Permission request reads on every surface: toast, inbox, cards, request card.
import type { Request } from "./schema/Request";

/** The heading word for each of the five kinds. */
export const KIND_LABEL: Record<Request["type"], string> = {
  command: "command",
  fileWrite: "file write",
  network: "network",
  mcpTool: "MCP tool",
  question: "question",
};

const VERB: Record<Request["type"], string> = {
  command: "wants to run",
  fileWrite: "wants to write",
  network: "wants to reach",
  mcpTool: "wants to use",
  question: "asks",
};

/** "wants to run", "asks"… */
export function verb(request: Request): string {
  return VERB[request.type];
}

/** The exact thing asked for, as the mono block shows it. */
export function detail(request: Request): string {
  switch (request.type) {
    case "command":
      return request.command;
    case "fileWrite":
      return request.path;
    case "network":
      return request.url ?? request.domain;
    case "mcpTool":
      return `${request.server} · ${request.tool}`;
    case "question":
      return request.question;
  }
}

/** One line for lists: "Wants to run: pnpm test". */
export function wants(request: Request): string {
  const v = verb(request);
  return `${v[0].toUpperCase()}${v.slice(1)}: ${detail(request)}`;
}

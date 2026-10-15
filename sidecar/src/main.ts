// The Claude Code sidecar: `node claude-sidecar.mjs --config <json>`, started by the core in
// the worktree. Protocol commands in on stdin, protocol events out on stdout (one JSON object
// per line), diagnostics on stderr. Set SWITCHYARD_RECORD_DIR to also write the run as a
// conformance fixture. `--manifest` prints the adapter manifest and exits: the core's
// compat check asks it before any session.

import { accessSync, constants } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { createInterface } from "node:readline";
import { query } from "@anthropic-ai/claude-agent-sdk";
import { drive, type SidecarConfig } from "./driver.ts";
import { MANIFEST } from "./translate.ts";

if (process.argv.includes("--manifest")) {
  process.stdout.write(JSON.stringify(MANIFEST) + "\n");
  process.exit(0);
}
const flag = process.argv.indexOf("--config");
if (flag < 0 || !process.argv[flag + 1]) {
  process.stderr.write("usage: claude-sidecar --config <json> | --manifest\n");
  process.exit(2);
}
const config: SidecarConfig = JSON.parse(process.argv[flag + 1]);
// The SDK's own platform binary is not bundled: run the user's installed CLI.
config.claude_path ??= findClaude();

/** `claude` on PATH or in the usual install locations (apps launched from Finder get a short PATH). */
function findClaude(): string | null {
  const dirs = [
    ...(process.env.PATH ?? "").split(delimiter),
    join(homedir(), ".local/bin"),
    join(homedir(), ".claude/local"),
    "/opt/homebrew/bin",
    "/usr/local/bin",
  ];
  for (const dir of dirs) {
    const candidate = join(dir, "claude");
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch {
      /* keep looking */
    }
  }
  return null;
}

await drive({
  config,
  commands: createInterface({ input: process.stdin }),
  emit: (event) => process.stdout.write(JSON.stringify(event) + "\n"),
  query: query as never,
  recordDir: process.env.SWITCHYARD_RECORD_DIR || undefined,
});
process.exit(0);

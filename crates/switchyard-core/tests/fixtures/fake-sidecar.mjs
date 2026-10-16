// A stand-in for the Claude Code sidecar that speaks the adapter protocol on stdio without
// any SDK or CLI: it reports its working directory and launch config (allowed/disallowed
// tools, MCP servers and Model, when it was given any), echoes prompts as a finished turn,
// writes unreadable lines on "garbage" and exits on "crash". "env A B" reports A and B as it and a child process see them.
import { createInterface } from "node:readline";
import { execFileSync } from "node:child_process";

const config = JSON.parse(process.argv[process.argv.indexOf("--config") + 1]);
const say = (event) => process.stdout.write(JSON.stringify(event) + "\n");

say({
  type: "session_init",
  external_session_id: "fake-1",
  permission_mode: config.permission_mode,
  cli_permission_mode: "fake",
  cli: { name: "fake-cli", version: "0.0.1" },
  model: null,
});
say({ type: "agent_text", text: `cwd=${process.cwd()}` });
if (config.allowed_tools?.length || config.disallowed_tools?.length)
  say({ type: "agent_text", text: `allowed=${config.allowed_tools.join(",")} disallowed=${config.disallowed_tools.join(",")}` });
if (config.mcp_servers && Object.keys(config.mcp_servers).length)
  say({ type: "agent_text", text: `mcp=${JSON.stringify(config.mcp_servers)}` });
if ("model" in config) say({ type: "agent_text", text: `model=${JSON.stringify(config.model)}` });
if (config.allow_bypass) say({ type: "agent_text", text: `mode=${config.permission_mode} allow_bypass` });

for await (const line of createInterface({ input: process.stdin })) {
  const command = JSON.parse(line);
  if (command.type === "send_text" && command.text === "crash") {
    process.stderr.write("fake sidecar: crashing as asked\n");
    process.exit(3);
  }
  if (command.type === "send_text" && command.text === "garbage") {
    process.stdout.write("this is not json\n");
    say({ type: "telepathy", thought: "hi" });
  }
  if (command.type === "send_text" && command.text.startsWith("env ")) {
    // What this process and a child it starts (as the CLI starts MCP servers) see.
    const names = command.text.slice(4).split(" ");
    const show = (env) => names.map((n) => `${n}=${env[n] ?? "<unset>"}`).join(" ");
    const child = execFileSync("/usr/bin/env", { encoding: "utf8" });
    const childEnv = Object.fromEntries(child.split("\n").filter(Boolean).map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]));
    say({ type: "agent_text", text: `self: ${show(process.env)}` });
    say({ type: "agent_text", text: `child: ${show(childEnv)}` });
    say({ type: "turn_complete" });
    continue;
  }
  if (command.type === "send_text") {
    say({ type: "agent_text", text: `you said: ${command.text}` });
    say({ type: "turn_complete" });
  }
}

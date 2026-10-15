// A stand-in for the Claude Code sidecar that speaks the adapter protocol on stdio without
// any SDK or CLI: it reports its working directory and launch config (allowed/disallowed
// tools, when it was given any), echoes prompts as a finished turn, writes unreadable lines
// on "garbage" and exits on "crash".
import { createInterface } from "node:readline";

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
  if (command.type === "send_text") {
    say({ type: "agent_text", text: `you said: ${command.text}` });
    say({ type: "turn_complete" });
  }
}

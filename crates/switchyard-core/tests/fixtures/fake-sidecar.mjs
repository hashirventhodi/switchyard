// A stand-in for the Claude Code sidecar that speaks the adapter protocol on stdio without
// any SDK or CLI: it reports its working directory and launch config, echoes prompts as a
// finished turn, and exits on demand.
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

for await (const line of createInterface({ input: process.stdin })) {
  const command = JSON.parse(line);
  if (command.type === "send_text" && command.text === "crash") {
    process.stderr.write("fake sidecar: crashing as asked\n");
    process.exit(3);
  }
  if (command.type === "send_text") {
    say({ type: "agent_text", text: `you said: ${command.text}` });
    say({ type: "turn_complete" });
  }
}

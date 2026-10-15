// Replay mode for the conformance suite: the translator behind a line-framed request/reply
// loop on stdio. No SDK, no CLI, no model. Requests, one JSON object per line:
//   {"manifest":true}      -> {"manifest":{"name","implements"}}
//   {"ingest":"<raw line>"} -> {"events":[...]}
//   {"command":{...}}      -> {"responses":[...]}
// A translator failure replies {"error":"..."}.
//
// Run with Node >= 22.18 directly from source: node sidecar/src/replay.ts

import { createInterface } from "node:readline";
import { ClaudeTranslator, MANIFEST } from "./translate.ts";

const translator = new ClaudeTranslator();
const reply = (value: unknown) => process.stdout.write(JSON.stringify(value) + "\n");

for await (const line of createInterface({ input: process.stdin })) {
  if (!line.trim()) continue;
  try {
    const request = JSON.parse(line);
    if (request.manifest) reply({ manifest: MANIFEST });
    else if ("ingest" in request) reply({ events: translator.ingest(request.ingest) });
    else if ("command" in request) reply({ responses: translator.command(request.command) });
    else reply({ error: `unknown request: ${line}` });
  } catch (e) {
    reply({ error: (e as Error).message });
  }
}

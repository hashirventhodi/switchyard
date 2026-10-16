// @vitest-environment node
// `node claude-sidecar.mjs --manifest`: how the core checks, before any session, that this
// adapter speaks its protocol version. Prints the manifest and exits; no SDK call, no CLI.
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";

const main = fileURLToPath(new URL("../src/main.ts", import.meta.url));

test("--manifest prints the adapter's name and implemented protocol range", () => {
  const out = execFileSync(process.execPath, [main, "--manifest"], { encoding: "utf8" });

  expect(JSON.parse(out)).toEqual({ name: "claude-code", implements: "^0.2" });
});

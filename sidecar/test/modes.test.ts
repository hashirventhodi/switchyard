// @vitest-environment node
// Permission modes in the sidecar: each protocol mode starts the CLI in its own mode
// (bypass only when the host allows it), set_permission_mode switches the live query, and
// below Full auto a PreToolUse hook sends dangerous commands (network, delete, migration,
// push) to the host even when the CLI's mode or allow rules would approve them.
import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { classes } from "../src/dangerous.ts";
import { drive, type QueryFn } from "../src/driver.ts";
import type { PermissionMode } from "../src/protocol.ts";

function fakeQuery() {
  const calls = { options: undefined as any, modes: [] as string[] };
  const query: QueryFn = ({ options }) => {
    calls.options = options;
    async function* run() {}
    return Object.assign(run(), {
      interrupt: async () => undefined,
      setPermissionMode: async (mode: string) => {
        calls.modes.push(mode);
      },
    }) as any;
  };
  return { query, calls };
}

function commands(...lines: object[]) {
  return (async function* () {
    for (const line of lines) yield JSON.stringify(line);
  })();
}

async function run(permission_mode: PermissionMode, extra: { allow_bypass?: boolean } = {}, ...cmds: object[]) {
  const { query, calls } = fakeQuery();
  await drive({ config: { cwd: "/wt", permission_mode, claude_path: null, ...extra }, commands: commands(...cmds), emit: () => {}, query });
  // Let the command loop finish.
  await new Promise((r) => setTimeout(r, 10));
  return calls;
}

test("each mode starts the CLI in its own mode", async () => {
  const expected: Record<PermissionMode, string> = {
    explore: "plan",
    ask_first: "default",
    auto_edit: "acceptEdits",
    full_auto: "auto",
    bypass: "bypassPermissions",
  };
  for (const [mode, cli] of Object.entries(expected)) {
    const calls = await run(mode as PermissionMode, { allow_bypass: mode === "bypass" });
    expect(calls.options.permissionMode, mode).toBe(cli);
  }
});

test("bypass is only made available when the host allows it (a declared sandbox)", async () => {
  expect((await run("full_auto")).options.allowDangerouslySkipPermissions).toBeFalsy();
  expect((await run("full_auto", { allow_bypass: true })).options.allowDangerouslySkipPermissions).toBe(true);
});

test("set_permission_mode switches the live query to the CLI's mode", async () => {
  const calls = await run("ask_first", {}, { type: "set_permission_mode", permission_mode: "auto_edit" });
  expect(calls.modes).toEqual(["acceptEdits"]);
});

const bash = (command: string) => ({ hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: { command } });

async function hookAnswer(calls: { options: any }, input: object) {
  const matchers = calls.options.hooks.PreToolUse as { matcher?: string; hooks: ((...a: any[]) => Promise<any>)[] }[];
  const hook = matchers.find((m) => !m.matcher || new RegExp(`^(${m.matcher})$`).test((input as any).tool_name));
  if (!hook) return {};
  return hook.hooks[0](input, "toolu_1", { signal: new AbortController().signal });
}

test("below Full auto, dangerous commands always reach the host", async () => {
  for (const mode of ["explore", "ask_first", "auto_edit"] as const) {
    const calls = await run(mode);
    const answer = await hookAnswer(calls, bash("git push origin main"));
    expect(answer.hookSpecificOutput, mode).toMatchObject({ hookEventName: "PreToolUse", permissionDecision: "ask" });
    expect(await hookAnswer(calls, bash("pnpm test")), mode).toEqual({});
  }
  const fullAuto = await run("full_auto");
  expect(await hookAnswer(fullAuto, bash("git push origin main"))).toEqual({});
});

test("the hook follows a mode switch", async () => {
  const calls = await run("full_auto", {}, { type: "set_permission_mode", permission_mode: "auto_edit" });
  expect((await hookAnswer(calls, bash("rm -rf build"))).hookSpecificOutput?.permissionDecision).toBe("ask");
});

test("classifies the vectors shared with the core", () => {
  const vectors = JSON.parse(readFileSync(new URL("./dangerous-commands.json", import.meta.url), "utf8"));
  for (const { command, classes: expected } of vectors) expect([...classes(command)], command).toEqual(expected);
});

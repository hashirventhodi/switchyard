// The New session modal: agent, where to run (a new worktree from base, or an existing
// free one), permission mode, initial prompt.
import { useEffect, useState } from "react";
import { slugBranch, type BranchKind } from "./branch";
import type { Command } from "./schema/Command";
import type { PermissionMode } from "./schema/PermissionMode";
import { sessionOn, type State } from "./state";
import { AgentProblem, agentProblem, type OpenLink } from "./Setup";
import { MODE_LABEL, STATUS_LABEL, withinCeiling } from "./status";

// Explore is the only mode allowed on a primary worktree (ADR-0005).
const MODES: PermissionMode[] = ["askFirst", "autoEdit", "fullAuto", "explore"];

/** How the modal opens: on a given worktree, in a given mode, or for a new `fix/` branch. */
export interface NewSessionPreset {
  worktreePath?: string;
  mode?: PermissionMode;
  repositoryId?: string;
  branchKind?: BranchKind;
}

interface Choice {
  path: string;
  label: string;
  disabled: boolean;
}

export function NewSession(props: {
  state: State;
  preset?: NewSessionPreset;
  onStart: (command: Command) => void;
  /** Reads the chosen repository's policy (its default mode and ceiling). */
  send: (command: Command) => void;
  onClose: () => void;
  openLink: OpenLink;
}) {
  const { state, preset = {}, onStart, send, onClose, openLink } = props;
  // An agent whose CLI is missing or outside this build's range cannot start (the core
  // refuses too); say why here, before the first session, not mid-session.
  const blocked = agentProblem(state.agents, "claudeCode");
  const [mode, setMode] = useState<PermissionMode>(preset.mode ?? "askFirst");
  // Until the user (or the preset) picks one, the mode follows the repository's default.
  const [modeTouched, setModeTouched] = useState(preset.mode !== undefined);
  const [where, setWhere] = useState<"new" | "existing">(preset.worktreePath ? "existing" : "new");
  const choices: Choice[] = state.repositories.flatMap((r) =>
    r.worktrees.map((w) => {
      const name = `${r.name} / ${w.branch ?? "(detached)"}`;
      const occupant = sessionOn(state, w.path);
      if (occupant) return { path: w.path, label: `${name} (${STATUS_LABEL[occupant.status]})`, disabled: true };
      // Only Explore sessions may run on a primary worktree.
      if (w.primary) return { path: w.path, label: `${name} (primary: Explore only)`, disabled: mode !== "explore" };
      return { path: w.path, label: name, disabled: false };
    }),
  );
  const [worktree, setWorktree] = useState(
    () => preset.worktreePath ?? choices.find((c) => !c.disabled)?.path ?? "",
  );
  const [repositoryId, setRepositoryId] = useState(preset.repositoryId ?? state.repositories[0]?.id ?? "");
  const [branch, setBranch] = useState("");
  const [branchTouched, setBranchTouched] = useState(false);
  const [prompt, setPrompt] = useState("");

  const repository = state.repositories.find((r) => r.id === repositoryId);
  const policyRepo =
    where === "new" ? repositoryId : state.repositories.find((r) => r.worktrees.some((w) => w.path === worktree))?.id;
  const policy = policyRepo ? state.policies[policyRepo] : undefined;
  const modes = MODES.filter((m) => withinCeiling(m, policy?.ceiling ?? null));
  useEffect(() => {
    if (policyRepo) send({ type: "readPolicy", repositoryId: policyRepo });
  }, [policyRepo, send]);
  useEffect(() => {
    if (!modeTouched && policy?.defaultMode) setMode(policy.defaultMode);
  }, [modeTouched, policy?.defaultMode]);
  // The core refuses a mode above the ceiling; never offer one.
  const shownMode = modes.includes(mode) ? mode : (modes.find((m) => m !== "explore") ?? "explore");
  const shownBranch = branchTouched ? branch : slugBranch(prompt, preset.branchKind);
  const chosen = choices.find((c) => c.path === worktree);
  const canStart =
    !blocked &&
    !!prompt.trim() && (where === "new" ? !!repository && !!shownBranch.trim() : !!chosen && !chosen.disabled);

  const start = () => {
    if (!canStart) return;
    const agent = "claudeCode";
    onStart(
      where === "new"
        ? { type: "startSessionOnNewWorktree", repositoryId, branch: shownBranch.trim(), agent, permissionMode: shownMode, prompt }
        : { type: "startSession", worktreePath: worktree, agent, permissionMode: shownMode, prompt },
    );
    onClose();
  };

  return (
    <div className="overlay-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <form
        className="sheet"
        role="dialog"
        aria-label="New session"
        aria-modal="true"
        onSubmit={(e) => {
          e.preventDefault();
          start();
        }}
      >
        <div className="modal-h">
          <h2>New session</h2>
          <button type="button" className="iconbtn" aria-label="Close" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-b">
          <label className="field">
            <span>Agent</span>
            <select aria-label="Agent" value="claudeCode" onChange={() => {}}>
              <option value="claudeCode">Claude Code</option>
            </select>
          </label>
          {blocked && (
            <div className="setup-alert" role="alert">
              <AgentProblem setup={blocked} openLink={openLink} />
            </div>
          )}
          <fieldset className="field choice">
            <legend>Where</legend>
            <label className={where === "new" ? "on" : ""}>
              <input type="radio" name="where" checked={where === "new"} onChange={() => setWhere("new")} />
              <b>Create new from base</b>
              <small>isolated branch, recommended</small>
            </label>
            <label className={where === "existing" ? "on" : ""}>
              <input type="radio" name="where" checked={where === "existing"} onChange={() => setWhere("existing")} />
              <b>Use existing</b>
              <small>{choices.filter((c) => !c.disabled).length} free</small>
            </label>
          </fieldset>
          {where === "new" ? (
            <div className="row2">
              <label className="field">
                <span>Repository</span>
                <select aria-label="Repository" value={repositoryId} onChange={(e) => setRepositoryId(e.target.value)}>
                  {state.repositories.map((r) => (
                    <option key={r.id} value={r.id}>
                      {r.name}
                    </option>
                  ))}
                </select>
              </label>
              <label className="field">
                <span>
                  Branch <span className="hint">from {repository?.baseBranch ?? "base"}</span>
                </span>
                <input
                  aria-label="Branch"
                  className="mono"
                  value={shownBranch}
                  placeholder={preset.branchKind ? `${preset.branchKind}/…` : "feat/…"}
                  onChange={(e) => {
                    setBranchTouched(true);
                    setBranch(e.target.value);
                  }}
                />
              </label>
            </div>
          ) : (
            <>
              <label className="field">
                <span>Worktree</span>
                <select aria-label="Worktree" value={worktree} onChange={(e) => setWorktree(e.target.value)}>
                  {choices.map((c) => (
                    <option key={c.path} value={c.path} disabled={c.disabled}>
                      {c.label}
                    </option>
                  ))}
                </select>
              </label>
              {choices.every((c) => c.disabled) && (
                <span className="hint">No free worktree. Create a new one from base instead.</span>
              )}
            </>
          )}
          <fieldset className="field seg">
            <legend>Permission mode</legend>
            {modes.map((m) => (
              <label key={m}>
                <input
                  type="radio"
                  name="mode"
                  value={m}
                  checked={shownMode === m}
                  onChange={() => {
                    setModeTouched(true);
                    setMode(m);
                  }}
                />
                {MODE_LABEL[m]}
              </label>
            ))}
            {policy?.ceiling && policy.ceiling !== "fullAuto" && (
              <span className="hint">This repository's policy allows at most {MODE_LABEL[policy.ceiling]}.</span>
            )}
          </fieldset>
          <label className="field">
            <span>Prompt</span>
            <textarea
              aria-label="Prompt"
              rows={5}
              value={prompt}
              placeholder="What should the agent do?"
              onChange={(e) => setPrompt(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault();
                  start();
                }
              }}
            />
          </label>
        </div>
        <div className="modal-f">
          <button type="button" className="btn ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn primary" disabled={!canStart}>
            Start session
          </button>
        </div>
      </form>
    </div>
  );
}

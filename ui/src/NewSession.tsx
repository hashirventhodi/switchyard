// The New session modal: agent and its Model, where to run (a new worktree from base, or an
// existing free one), permission mode, initial prompt.
import { useEffect, useState } from "react";
import { slugBranch, type BranchKind } from "./branch";
import type { AgentKind } from "./schema/AgentKind";
import type { Command } from "./schema/Command";
import type { ModelChoice } from "./schema/ModelChoice";
import type { PermissionMode } from "./schema/PermissionMode";
import type { Task } from "./schema/Task";
import { taskBranch, taskPrompt } from "./tasks";
import { prefillKey, sessionOn, type State } from "./state";
import { AgentProblem, agentProblem, type OpenLink } from "./Setup";
import { AGENT_LABEL, AGENTS, MODE_LABEL, STATUS_LABEL, withinCeiling } from "./status";
import { choosesModel, DEFAULT_MODEL } from "./model";
import { ModelField } from "./ModelField";

// Explore is the only mode allowed on a primary worktree (ADR-0005).
const MODES: PermissionMode[] = ["askFirst", "autoEdit", "fullAuto", "explore"];

/** How the modal opens: on a given worktree, in a given mode, or for a new `fix/` branch. */
export interface NewSessionPreset {
  worktreePath?: string;
  mode?: PermissionMode;
  repositoryId?: string;
  branchKind?: BranchKind;
  /** Assign to agent: the prompt and branch come from the task, and the session links to it. */
  task?: Task;
  /** The task's repository's house rules, appended to its prompt. */
  houseRules?: string;
}

interface Choice {
  path: string;
  label: string;
  disabled: boolean;
  /** The Paused, Finished or Error session starting here would stop and replace (archive). */
  replaces?: { id: string; status: string };
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
  const [agent, setAgent] = useState<AgentKind>("claudeCode");
  const blocked = agentProblem(state.agents, agent);
  // Until the user picks one, the Model is the last choice for this agent in this Repository.
  const [modelChosen, setModelChosen] = useState<ModelChoice | null>(null);
  const [mode, setMode] = useState<PermissionMode>(preset.mode ?? "askFirst");
  // Until the user (or the preset) picks one, the mode follows the repository's default.
  const [modeTouched, setModeTouched] = useState(preset.mode !== undefined);
  const [where, setWhere] = useState<"new" | "existing">(preset.worktreePath ? "existing" : "new");
  const task = preset.task;
  // A task's session runs in the task's repository.
  const repositories = task ? state.repositories.filter((r) => r.id === task.repositoryId) : state.repositories;
  const choices: Choice[] = repositories.flatMap((r) =>
    r.worktrees.map((w) => {
      const name = `${r.name} / ${w.branch ?? "(detached)"}`;
      const occupant = sessionOn(state, w.path);
      const label = STATUS_LABEL[occupant?.status ?? "idle"];
      // An agent at work is never replaced; a Paused, Finished or Error one may be (archived).
      if (occupant && (occupant.status === "running" || occupant.status === "waiting"))
        return { path: w.path, label: `${name} (${label})`, disabled: true };
      const replaces = occupant ? { id: occupant.id, status: label } : undefined;
      const suffix = occupant ? ` (${label}: stop and replace)` : "";
      // Only Explore sessions may run on a primary worktree.
      if (w.primary)
        return { path: w.path, label: `${name} (primary: Explore only)${suffix}`, disabled: mode !== "explore", replaces };
      return { path: w.path, label: name + suffix, disabled: false, replaces };
    }),
  );
  const [worktree, setWorktree] = useState(
    () => preset.worktreePath ?? choices.find((c) => !c.disabled && !c.replaces)?.path ?? choices.find((c) => !c.disabled)?.path ?? "",
  );
  const [repositoryId, setRepositoryId] = useState(
    task?.repositoryId ?? preset.repositoryId ?? state.repositories[0]?.id ?? "",
  );
  // A task names the branch `<kind>/<key>-<slug>`; otherwise it follows the prompt until typed.
  const [branch, setBranch] = useState(task ? taskBranch(task) : "");
  const [branchTouched, setBranchTouched] = useState(!!task);
  const [prompt, setPrompt] = useState(task ? taskPrompt(task, preset.houseRules) : "");

  const repository = state.repositories.find((r) => r.id === repositoryId);
  const policyRepo =
    where === "new" ? repositoryId : state.repositories.find((r) => r.worktrees.some((w) => w.path === worktree))?.id;
  const policy = policyRepo ? state.policies[policyRepo] : undefined;
  const modes = MODES.filter((m) => withinCeiling(m, policy?.ceiling ?? null));
  const prefill = policyRepo && !modelChosen ? state.modelPrefills[prefillKey(agent, policyRepo)] : undefined;
  // An agent whose Model cannot be chosen yet starts on Default.
  const model = choosesModel(agent) ? (modelChosen ?? prefill?.choice ?? DEFAULT_MODEL) : DEFAULT_MODEL;
  useEffect(() => {
    if (policyRepo) send({ type: "readPolicy", repositoryId: policyRepo });
  }, [policyRepo, send]);
  // The cached Model list, at once; the core refreshes it in the background and pushes a
  // fresher one if it arrives while the dialog is open.
  useEffect(() => {
    send({ type: "readModels", agent });
  }, [agent, send]);
  // The last Model choice for this agent here, remembered by the core (never in the Repository).
  useEffect(() => {
    if (policyRepo) send({ type: "readModelPrefill", agent, repositoryId: policyRepo });
  }, [agent, policyRepo, send]);
  useEffect(() => {
    if (!modeTouched && policy?.defaultMode) setMode(policy.defaultMode);
  }, [modeTouched, policy?.defaultMode]);
  // The core refuses a mode above the ceiling; never offer one.
  const shownMode = modes.includes(mode) ? mode : (modes.find((m) => m !== "explore") ?? "explore");
  const shownBranch = branchTouched ? branch : slugBranch(prompt, preset.branchKind);
  const chosen = choices.find((c) => c.path === worktree);
  const replacing = where === "existing" ? chosen?.replaces : undefined;
  const canStart =
    !blocked &&
    !!prompt.trim() && (where === "new" ? !!repository && !!shownBranch.trim() : !!chosen && !chosen.disabled);

  const start = () => {
    if (!canStart) return;
    if (task) {
      const target =
        where === "new"
          ? ({ type: "newWorktree", branch: shownBranch.trim() } as const)
          : replacing
            ? ({ type: "replace", sessionId: replacing.id } as const)
            : ({ type: "worktree", worktreePath: worktree } as const);
      onStart({ type: "startTaskSession", taskId: task.id, target, agent, permissionMode: shownMode, model, prompt });
      return onClose();
    }
    onStart(
      where === "new"
        ? { type: "startSessionOnNewWorktree", repositoryId, branch: shownBranch.trim(), agent, permissionMode: shownMode, model, prompt }
        : replacing
          ? { type: "replaceSession", sessionId: replacing.id, agent, permissionMode: shownMode, model, prompt }
          : { type: "startSession", worktreePath: worktree, agent, permissionMode: shownMode, model, prompt },
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
            <select aria-label="Agent" value={agent} onChange={(e) => setAgent(e.target.value as AgentKind)}>
              {AGENTS.map((a) => (
                <option key={a} value={a}>
                  {AGENT_LABEL[a]}
                </option>
              ))}
            </select>
          </label>
          {choosesModel(agent) && <ModelField value={model} onChange={setModelChosen} list={state.modelLists[agent]} />}
          {choosesModel(agent) && prefill?.retired && (
            <span className="hint" role="note" aria-label="Remembered Model">
              {prefill.retired}, your last choice here, is no longer offered: Default instead
            </span>
          )}
          {task && (
            <div className="field" role="group" aria-label="Task">
              <span>Task</span>
              <span className="task-line">
                {task.key} · {task.title}
              </span>
            </div>
          )}
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
              <small>{choices.filter((c) => !c.disabled && !c.replaces).length} free</small>
            </label>
          </fieldset>
          {where === "new" ? (
            <div className="row2">
              <label className="field">
                <span>Repository</span>
                <select aria-label="Repository" value={repositoryId} onChange={(e) => setRepositoryId(e.target.value)}>
                  {repositories.map((r) => (
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
              {replacing && (
                <span className="hint" role="note">
                  The {replacing.status} session there stops and is archived; its transcript stays readable in Context.
                </span>
              )}
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
          <button type="submit" className={`btn ${replacing ? "danger" : "primary"}`} disabled={!canStart}>
            {replacing ? "Stop and replace" : "Start session"}
          </button>
        </div>
      </form>
    </div>
  );
}

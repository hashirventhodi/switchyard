// The New session modal: agent, an existing worktree, permission mode, initial prompt.
import { useState } from "react";
import type { Command } from "./schema/Command";
import type { PermissionMode } from "./schema/PermissionMode";
import { sessionOn, type State } from "./state";
import { MODE_LABEL, STATUS_LABEL } from "./status";

// Only Ask first is available until permission modes land; Explore (the only mode allowed
// on a primary worktree) arrives with them.
const MODES: { mode: PermissionMode; available: boolean }[] = [
  { mode: "askFirst", available: true },
  { mode: "autoEdit", available: false },
  { mode: "fullAuto", available: false },
  { mode: "explore", available: false },
];

interface Choice {
  path: string;
  label: string;
  disabled: boolean;
}

export function NewSession(props: { state: State; onStart: (command: Command) => void; onClose: () => void }) {
  const { state, onStart, onClose } = props;
  const choices: Choice[] = state.repositories.flatMap((r) =>
    r.worktrees.map((w) => {
      const name = `${r.name} / ${w.branch ?? "(detached)"}`;
      const occupant = sessionOn(state, w.path);
      if (w.primary) return { path: w.path, label: `${name} (primary: Explore only)`, disabled: true };
      if (occupant) return { path: w.path, label: `${name} (${STATUS_LABEL[occupant.status]})`, disabled: true };
      return { path: w.path, label: name, disabled: false };
    }),
  );
  const [worktree, setWorktree] = useState(() => choices.find((c) => !c.disabled)?.path ?? "");
  const [mode, setMode] = useState<PermissionMode>("askFirst");
  const [prompt, setPrompt] = useState("");
  const canStart = !!worktree && !!prompt.trim();

  const start = () => {
    if (!canStart) return;
    onStart({ type: "startSession", worktreePath: worktree, agent: "claudeCode", permissionMode: mode, prompt });
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
            <span className="hint">No free worktree. Create one with git worktree add, then add it here.</span>
          )}
          <fieldset className="field seg">
            <legend>Permission mode</legend>
            {MODES.map(({ mode: m, available }) => (
              <label key={m} className={available ? "" : "off"} title={available ? undefined : "Arrives with permission modes"}>
                <input
                  type="radio"
                  name="mode"
                  value={m}
                  disabled={!available}
                  checked={mode === m}
                  onChange={() => setMode(m)}
                />
                {MODE_LABEL[m]}
              </label>
            ))}
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

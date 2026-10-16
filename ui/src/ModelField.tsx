// New session's Model picker: Default first, then the Models the CLI listed (from the core's
// cache). While there is no list, or the CLI cannot list, it is free text (blank for
// Default) with a note saying why. A list that arrives after the user typed a name never
// replaces it: the typed text stays, with the list beside it.
import { useState } from "react";
import type { ListedModel } from "./schema/ListedModel";
import type { ModelChoice } from "./schema/ModelChoice";
import type { ModelList } from "./schema/ModelList";
import { modelChoice } from "./model";

export function ModelField(props: { value: ModelChoice; onChange: (choice: ModelChoice) => void; list?: ModelList }) {
  const { value, onChange, list } = props;
  // Once the user types a name, the field stays free text for as long as the dialog is open.
  const [typed, setTyped] = useState(false);
  const models = list?.type === "listed" && list.models.length ? list.models : null;
  const text = (
    <input
      aria-label="Model"
      className="mono"
      placeholder="Default"
      value={value.type === "model" ? value.id : ""}
      onChange={(e) => {
        setTyped(true);
        onChange(modelChoice(e.target.value));
      }}
    />
  );

  if (!models) {
    return (
      <label className="field">
        <span>
          Model <span className="hint">blank for Default: the CLI, or your own CLI settings, decide</span>
        </span>
        {text}
        <span className="hint" role="note" aria-label="Model list">
          {listNote(list)}
        </span>
      </label>
    );
  }
  if (typed && value.type === "model") {
    return (
      <div className="field">
        <span>Model</span>
        <div className="row2">
          {text}
          <ModelSelect label="Listed Models" models={models} value={value} onChange={onChange} />
        </div>
      </div>
    );
  }
  return (
    <label className="field">
      <span>
        Model <span className="hint">Default: the CLI, or your own CLI settings, decide</span>
      </span>
      <ModelSelect label="Model" models={models} value={value} onChange={onChange} />
    </label>
  );
}

function ModelSelect(props: {
  label: string;
  models: ListedModel[];
  value: ModelChoice;
  onChange: (choice: ModelChoice) => void;
}) {
  const { label, models, value, onChange } = props;
  const id = value.type === "model" ? value.id : "";
  // A chosen Model the list does not name is still shown as chosen, never silently swapped.
  const unlisted = id && !models.some((m) => m.id === id);
  return (
    <select aria-label={label} value={id} onChange={(e) => onChange(modelChoice(e.target.value))}>
      <option value="">Default</option>
      {models.map((m) => (
        <option key={m.id} value={m.id}>
          {m.displayName}
        </option>
      ))}
      {unlisted && <option value={id}>{id}</option>}
    </select>
  );
}

/** Why the Model is free text: no list cached for this CLI yet, or the CLI cannot list. */
function listNote(list: ModelList | undefined): string {
  if (list?.type === "cannotList") {
    const why = list.reason ? ` (${list.reason})` : "";
    return `This CLI did not list its Models${why}: type a Model id, or leave it blank for Default`;
  }
  return "Model list loads after the first session with this CLI";
}

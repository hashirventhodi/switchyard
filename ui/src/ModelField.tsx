// New session's Model field: the shared Model picker ([ModelPicker]) with Default first,
// labelled for the dialog. While there is no list, or the CLI cannot list, it is free text
// (blank for Default) with a note saying why.
import type { ModelChoice } from "./schema/ModelChoice";
import type { ModelList } from "./schema/ModelList";
import { modelChoice } from "./model";
import { listedModels, ModelPicker } from "./ModelPicker";

export function ModelField(props: { value: ModelChoice; onChange: (choice: ModelChoice) => void; list?: ModelList }) {
  const { value, onChange, list } = props;
  const picker = (
    <ModelPicker
      value={value.type === "model" ? value.id : ""}
      onChange={(id) => onChange(modelChoice(id))}
      list={list}
      offersDefault
      placeholder="Default"
    />
  );
  // One structure either way, so the picker (and what the user typed) survives a list arriving.
  const listed = listedModels(list);
  return (
    <div className="field">
      <span>
        Model{" "}
        <span className="hint">
          {listed ? "Default: the CLI, or your own CLI settings, decide" : "blank for Default: the CLI, or your own CLI settings, decide"}
        </span>
      </span>
      {picker}
      {!listed && (
        <span className="hint" role="note" aria-label="Model list">
          {listNote(list)}
        </span>
      )}
    </div>
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

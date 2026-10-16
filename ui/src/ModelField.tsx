// New session's Model field: Default (left blank) or a Model id typed in.
import type { ModelChoice } from "./schema/ModelChoice";
import { modelChoice } from "./model";

export function ModelField(props: { value: ModelChoice; onChange: (choice: ModelChoice) => void }) {
  const { value, onChange } = props;
  return (
    <label className="field">
      <span>
        Model <span className="hint">blank for Default: the CLI, or your own CLI settings, decide</span>
      </span>
      <input
        aria-label="Model"
        className="mono"
        placeholder="Default"
        value={value.type === "model" ? value.id : ""}
        onChange={(e) => onChange(modelChoice(e.target.value))}
      />
    </label>
  );
}

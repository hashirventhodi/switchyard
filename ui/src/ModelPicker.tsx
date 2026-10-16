// The Model picker New session and the composer's Model switch share: the Models the CLI
// listed (from the core's cache) as a dropdown, else free text. A list that arrives after
// the user typed a name never replaces it: the typed text stays, with the list beside it.
// A chosen id that names a listed Model by the full id its alias stands for ("claude-opus-5-5"
// for "opus") shows as that Model.
import { useState } from "react";
import type { ListedModel } from "./schema/ListedModel";
import type { ModelList } from "./schema/ModelList";

/** The listed Models, when there is a list to pick from. */
export function listedModels(list: ModelList | undefined): ListedModel[] | null {
  return list?.type === "listed" && list.models.length ? list.models : null;
}

/** Whether `a` and `b` name one Model: equal, or a listed Model's id and the full id it resolves to. */
export function sameModel(list: ModelList | undefined, a: string, b: string): boolean {
  if (a === b) return true;
  const is = (m: ListedModel, id: string) => m.id === id || m.resolvedModel === id;
  return (listedModels(list) ?? []).some((m) => is(m, a) && is(m, b));
}

export function ModelPicker(props: {
  /** A Model id; "" for none (Default, where it is offered). */
  value: string;
  onChange: (id: string) => void;
  list?: ModelList;
  /** "Default" (blank) is offered first: New session's choice, never a switch. */
  offersDefault: boolean;
  /** What the free-text field shows while empty, and the dropdown while nothing is chosen. */
  placeholder: string;
}) {
  const { value, onChange, list, offersDefault, placeholder } = props;
  // Once the user types a name, the picker stays free text for as long as it is shown.
  const [typed, setTyped] = useState(false);
  const models = listedModels(list);
  const text = (
    <input
      aria-label="Model"
      className="mono"
      placeholder={placeholder}
      value={value}
      onChange={(e) => {
        setTyped(true);
        onChange(e.target.value);
      }}
    />
  );
  if (!models) return text;
  const select = (label: string) => (
    <ModelSelect label={label} models={models} value={value} onChange={onChange} offersDefault={offersDefault} placeholder={placeholder} />
  );
  if (typed && value.trim()) {
    return (
      <div className="row2">
        {text}
        {select("Listed Models")}
      </div>
    );
  }
  return select("Model");
}

function ModelSelect(props: {
  label: string;
  models: ListedModel[];
  value: string;
  onChange: (id: string) => void;
  offersDefault: boolean;
  placeholder: string;
}) {
  const { label, models, value, onChange, offersDefault, placeholder } = props;
  const id = value.trim();
  const listed = models.find((m) => m.id === id || m.resolvedModel === id);
  // A chosen Model the list does not name is still shown as chosen, never silently swapped.
  const unlisted = id && !listed;
  return (
    <select aria-label={label} value={listed?.id ?? id} onChange={(e) => onChange(e.target.value)}>
      {offersDefault ? (
        <option value="">Default</option>
      ) : (
        !id && (
          <option value="" disabled>
            {placeholder}
          </option>
        )
      )}
      {models.map((m) => (
        <option key={m.id} value={m.id}>
          {m.displayName}
        </option>
      ))}
      {unlisted && <option value={id}>{id}</option>}
    </select>
  );
}

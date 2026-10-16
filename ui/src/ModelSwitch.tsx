// The composer's Model control: switch a running Session's Model, picked from the CLI's
// cached Model list as in New session (the shared [ModelPicker]; free text without a list).
// A switch applies from the agent's next request and shows as pending until the core confirms the agent runs on it; a
// switch that did not take shows why and the control goes back to the Model running. On a
// CLI that cannot switch `live` it reads "applies on resume": the switch is recorded, the
// running process is left alone, and the Session resumes on it.
import { useEffect, useState } from "react";
import type { Command } from "./schema/Command";
import type { ModelList } from "./schema/ModelList";
import type { SessionView } from "./state";
import { ModelPicker, sameModel } from "./ModelPicker";

export function ModelSwitch(props: { session: SessionView; send: (command: Command) => void; live: boolean; list?: ModelList }) {
  const { session, send, live, list } = props;
  const running = session.facts.model ?? "";
  const pending = session.pendingModel ?? null;
  const failed = session.modelSwitchFailed ?? null;
  // What the control shows: the pending switch, else the Model running (also after a failure).
  const shown = pending ?? running;
  const [text, setText] = useState(shown);
  useEffect(() => setText(shown), [shown]);
  const model = text.trim();
  // An alias of the Model shown ("opus" for claude-opus-5-5) is no switch.
  const switches = !!model && !sameModel(list, model, shown);
  const submit = () => {
    if (switches) send({ type: "setModel", sessionId: session.id, model });
  };
  return (
    <form
      className="model-switch"
      role="group"
      aria-label="Session Model"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <ModelPicker value={text} onChange={setText} list={list} offersDefault={false} placeholder={running || "Default (not reported)"} />
      <button className="btn" type="submit" disabled={!switches}>
        Switch
      </button>
      {!live && (
        <span role="note" className="model-switch-note">
          Applies on resume
        </span>
      )}
      {pending && (
        <span role="status" className="model-switch-note">
          {live ? `Switching to ${pending} from the agent's next request…` : `Switches to ${pending} when the session resumes.`}
        </span>
      )}
      {failed && !pending && (
        <span role="alert" className="model-switch-note failed">
          {failureText(failed)}
        </span>
      )}
    </form>
  );
}

/** The core's reason for a failed switch, as it words it (the CLI's message, or why the
 * agent's next request shows the switch did not take). */
function failureText(failed: { model: string; reason: string | null }): string {
  const why = failed.reason?.replace(/\.$/, "") || "it did not take";
  return `Could not switch to ${failed.model}: ${why}.`;
}

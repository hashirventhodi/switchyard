// The composer's Model control: switch a running Session's Model. A switch applies from the
// agent's next request and shows as pending until the core confirms the agent runs on it; a
// switch that did not take shows why and the control goes back to the Model running. On a
// CLI that cannot switch `live` it reads "applies on resume": the switch is recorded, the
// running process is left alone, and the Session resumes on it.
import { useEffect, useState } from "react";
import type { Command } from "./schema/Command";
import type { SessionView } from "./state";

export function ModelSwitch(props: { session: SessionView; send: (command: Command) => void; live: boolean }) {
  const { session, send, live } = props;
  const running = session.facts.model ?? "";
  const pending = session.pendingModel ?? null;
  const failed = session.modelSwitchFailed ?? null;
  // What the control shows: the pending switch, else the Model running (also after a failure).
  const shown = pending ?? running;
  const [text, setText] = useState(shown);
  useEffect(() => setText(shown), [shown]);
  const model = text.trim();
  const submit = () => {
    if (model && model !== shown) send({ type: "setModel", sessionId: session.id, model });
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
      <input
        aria-label="Model"
        className="mono"
        placeholder={running || "Default (not reported)"}
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      <button className="btn" type="submit" disabled={!model || model === shown}>
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

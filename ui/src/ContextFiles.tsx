// Context's Instructions and Memory sections: the files the session's agent reads as
// instructions in the worktree (CLAUDE.md, AGENTS.md…) and the memory Claude Code keeps for
// it. Read through the core, shown read-only; edit them in the repository or the CLI.
import { useState } from "react";
import type { ChangeKind } from "./schema/ChangeKind";
import type { Command } from "./schema/Command";
import type { ContextFile } from "./schema/ContextFile";
import type { ContextKind } from "./schema/ContextKind";
import type { SessionView, State } from "./state";
import { useWorktreeChanged } from "./useWorktreeChanged";

/** The instruction and memory files are configuration on disk. */
const CONTEXT_KINDS: ChangeKind[] = ["config"];

export function ContextFiles(props: { state: State; session: SessionView; send: (command: Command) => void }) {
  const { state, session, send } = props;
  const path = session.worktreePath;
  useWorktreeChanged(state.worktreeChanges, path, CONTEXT_KINDS, () => send({ type: "readContext", worktreePath: path }));
  const files = state.contextFiles[path];
  return (
    <>
      <FileList label="Instructions" kind="instructions" files={files} empty="No CLAUDE.md or AGENTS.md in this worktree." />
      <FileList label="Memory" kind="memory" files={files} empty="No memory kept for this worktree." />
    </>
  );
}

function FileList(props: { label: string; kind: ContextKind; files: ContextFile[] | undefined; empty: string }) {
  const { label, kind, files, empty } = props;
  const [open, setOpen] = useState<string | null>(null);
  const list = (files ?? []).filter((f) => f.kind === kind);
  return (
    <section className="ctx context-files" aria-label={label}>
      <h4>
        {label}
        {list.length > 0 && <span>{list.length}</span>}
      </h4>
      {files === undefined ? (
        <p className="empty">Reading…</p>
      ) : list.length === 0 ? (
        <p className="empty">{empty}</p>
      ) : (
        <ul>
          {list.map((f) => (
            <li key={f.path} className="cf">
              <button
                className="kv"
                aria-label={`${open === f.path ? "Hide" : "Show"} ${f.name}`}
                aria-expanded={open === f.path}
                title={f.path}
                onClick={() => setOpen((o) => (o === f.path ? null : f.path))}
              >
                <span className="k">{f.name}</span>
                <span className="v">{lines(f.text)}</span>
              </button>
              {open === f.path && (
                <>
                  <pre className="cf-text">{f.text}</pre>
                  {f.truncated && <p className="note">Only the start of this file is shown.</p>}
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function lines(text: string): string {
  const n = text.split("\n").filter((l, i, all) => l !== "" || i < all.length - 1).length;
  return `${n} line${n === 1 ? "" : "s"}`;
}

// The one rule for a view that shows heavy data for a worktree (ADR-0008): read it on open,
// and re-read it whenever the core signals `worktreeChanged` for that worktree with a kind
// the view shows. The renderer never decides on its own when data is stale.
import { useEffect, useRef } from "react";
import type { ChangeKind } from "./schema/ChangeKind";
import type { State } from "./state";

/** Calls `read` when mounted (or the worktree changes) and again on every `worktreeChanged`
 *  signal for `worktreePath` carrying any of `kinds`. */
export function useWorktreeChanged(
  changes: State["worktreeChanges"],
  worktreePath: string,
  kinds: readonly ChangeKind[],
  read: () => void,
) {
  const counts = changes[worktreePath];
  // How many matching signals this worktree has had: moves exactly when a re-read is due.
  const seen = kinds.reduce((n, kind) => n + (counts?.[kind] ?? 0), 0);
  const latest = useRef(read);
  latest.current = read;
  useEffect(() => latest.current(), [worktreePath, seen]);
}

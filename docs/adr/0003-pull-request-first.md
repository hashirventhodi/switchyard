# Work leaves a worktree through a pull request, not a local merge

The prototype merges a worktree's branch into base in the primary checkout and deletes the worktree. That bypasses branch protection, CI and merge queues and leaves the user's main diverged from the remote. We decided the shipping action is "Open pull request": push, open or enqueue a PR on the forge, then poll until the forge reports the branch Merged, at which point the worktree is cleaned up and the task moves to Done. Local merge exists only for repositories with no remote.

## Consequences

- Cleanup is triggered by the forge's state, so merges done outside Switchyard are handled the same way.
- The M2 gate reads: a session can be started and its PR opened without a terminal, and the worktree is cleaned when the remote reports the merge.
- `gh` is a runtime dependency detected at startup, with an explicit setup state when missing or unauthenticated. GitHub is the only forge implementation through M4, behind a Forge interface.

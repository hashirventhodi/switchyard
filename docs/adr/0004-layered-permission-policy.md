# Permission grants are Switchyard data, layered org → repo → user, never written into CLI settings

The prototype stored "Always allow" on the session; the handoff proposed the worktree, and also proposed writing grants through to each CLI's own settings. We decided grants are repo-scoped Switchyard data in three layers (organisation policy, the repository's committed `.switchyard/policy.yml`, the user's local grants for that repository), deny wins across layers, and the resolved policy is translated into each CLI's flags at spawn so the CLI stops asking for what is already granted. The stored truth never lives in a CLI's settings file.

## Considered options

- Worktree scope: multiplies prompts and state for no change in meaning; `pnpm` means the same on every branch.
- Writing into CLI settings: one policy engine could not serve all CLIs, and grants would be invisible to audit.

## Consequences

- Patterns are a command prefix plus a glob on the remaining argument string; no shell parsing. A fixed list of interpreter prefixes can never be matched by a wildcard.
- The repository policy carries a default permission mode and a ceiling (for example, forbidding Full auto).
- In M1 only the repo file and user-local layers are built; the org layer is modelled but not synced.

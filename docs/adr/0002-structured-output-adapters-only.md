# Agent adapters consume structured output only, never terminal scraping

Each agent CLI could be driven through its interactive TUI in a PTY and parsed from ANSI output, which is what the prototype's visual design implied. We decided adapters only ever consume a CLI's structured mode (Claude Code via the Agent SDK in a Node sidecar, Codex via `app-server` JSON-RPC) and map it into one versioned adapter protocol with a recorded-session conformance suite. CLIs without a structured mode (Gemini CLI, Aider at the time of writing) are not supported until they have one or a vendor or community adapter targets the protocol.

## Consequences

- There is no agent PTY. The dock shows a read-only agent log derived from the event stream; "take over the terminal" is not a feature.
- The Claude Code control channel is not a documented public CLI protocol, so we use the vendor SDK rather than a reverse-engineered client. Revisit only if Anthropic documents the channel as stable.

# The adapter protocol is its own repository; fixtures belong to adapters; crates.io waits for a captured run

An Adapter is a pure translator: it never owns a process and is deterministic over the lines it is given, which is the only reason a Fixture can stand in for a CLI. We moved the Adapter protocol, its conformance checker and the `echo` reference adapter into a separate public repository (`switchyard-adapter-protocol`, MIT OR Apache-2.0), so that property is enforced by dependency direction: the protocol cannot reach into Switchyard, and a third-party adapter needs nothing of Switchyard's to prove itself. Fixtures for a real CLI belong to the adapter that translates it and stay beside it (Claude Code and Codex in Switchyard). The protocol is consumed by git tag until one adapter passes against a captured Fixture; only then is 0.1.0 published to crates.io, because synthesised Fixtures prove the types, not the CLI, and a crates.io version is permanent.

## Considered options

- Move every fixture into the protocol repository (the ticket as first written): each new CLI version would need a protocol release, and Switchyard would read its own adapters' fixtures out of the cargo registry.
- Publish a frozen copy of the Claude Code fixtures as an example: two copies that drift.
- Publish to crates.io now, or reserve the name with a placeholder: pins third parties to a version no real CLI run has exercised.

## Consequences

- One conformance checker with two front ends: in-process for Rust adapters, and the stdio `adapter-conformance` binary (one request, one response, in order; the manifest exchange is the version handshake) for everything else. The binary emits a JSON conformance report.
- The fixture format is versioned with the protocol, and every fixture's `meta.json` says whether it was captured or synthesised; synthesised fixtures never count toward conformance. The `echo` fixtures are the runner's self-test, not a conformance claim.
- The JSON Schema is generated from the Rust types; the sidecar generates its TypeScript from it at build time (not committed), and each tag carries `schema.json` as a release asset.

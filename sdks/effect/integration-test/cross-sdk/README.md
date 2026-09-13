# Effect cross-SDK acceptance fixture

This independent app exercises generated guest bridges in both directions:

- TypeScript and Rust peers consume metadata emitted by `EffectFixture`.
- `EffectFixture` exposes asymmetric nested input/output, a typed `EMPTY_ITEMS` failure, and `prefix` config.
- Both peers apply RPC config overrides through generated clients.
- The Effect consumer uses local definitions matching the peer metadata to call both providers.
- The Effect consumer discovers `TsPeer`, validates its reflected `echo` input schema, and invokes it through the runtime reflection client.
- The Effect consumer creates and invokes a reflected ephemeral TypeScript agent and checks its per-invocation identity.
- TypeScript and Rust callers assert the Effect provider's typed `EMPTY_ITEMS` failure payload.
- TypeScript ↔ Effect nested P3 streams assert asymmetric values, bounded pulls, and early reader closure.
- Rust ↔ Effect nested P3 streams use the generated Rust guest bridge and assert asymmetric values, bounded pulls, and early output closure.
- A TypeScript caller mutates an Effect snapshot-enabled agent, verifies the platform snapshot entry, manually updates the agent, and observes restored state through the generated TypeScript guest bridge.
- Reflection value calls preserve native TypeScript/Rust `NaN`, while JSON unpacking rejects it.
- Reflected invoke/schedule metadata is checked and a future TypeScript invocation is canceled before execution.
- A TypeScript generated guest client sends an asymmetric rich-schema corpus to Effect; Effect mutates selected fields and TypeScript asserts independent concrete results, including 64-bit boundaries.
- A TypeScript reflection client uses direct schema-value transport for Effect text, binary, quantity, and recursive schema nodes that are not viable through the shared generated TypeScript bridge surface.

The app deliberately uses ports `9892`, `9018`, and `9019`; it does not share the parent integration app's ports or deployment.

First rebuild the local TypeScript and Effect SDKs, including their template WASMs, then run `npm install` and `npm test`. The driver points all three languages at the co-located SDKs, without an npm fallback.

`npm test` builds only. Set `RUN_RUNTIME=1` to start a disposable local server, deploy, and assert all three RPC directions. The driver refuses an occupied server port and stops its own server on completion. Set `GOLEM_BIN` to select a current source-built binary; otherwise it resolves `golem` from PATH. SDK dependencies and tsconfig files must already be installed.

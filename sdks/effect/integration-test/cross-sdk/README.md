# Effect cross-SDK acceptance fixture

This independent app exercises generated guest bridges in both directions:

- TypeScript and Rust peers consume metadata emitted by `EffectFixture`.
- `EffectFixture` exposes asymmetric nested input/output, a typed `EMPTY_ITEMS` failure, and `prefix` config.
- Both peers apply RPC config overrides through generated clients.
- The Effect consumer uses local definitions matching the peer metadata to call both providers.

The app deliberately uses ports `9892`, `9018`, and `9019`; it does not share the parent integration app's ports or deployment.

First rebuild the local TypeScript and Effect SDKs, including their template WASMs, then run `npm install` and `npm test`. The driver points all three languages at the co-located SDKs, without an npm fallback.

`npm test` builds only. Set `RUN_RUNTIME=1` to start a disposable local server, deploy, and assert all three RPC directions. The driver refuses an occupied server port and stops its own server on completion. Set `GOLEM_BIN` to select a current source-built binary; otherwise it resolves `golem` from PATH.

The build currently skips CLI application checks because TypeScript and Effect embed different skills under the same name. SDK dependencies and tsconfig files must already be installed. This limitation does not skip component compilation or runtime assertions.

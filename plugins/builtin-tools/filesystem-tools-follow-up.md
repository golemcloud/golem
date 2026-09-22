# Filesystem tools follow-up status

The implementation keeps the single full `agent-guest` world and its external ABI. It does not
add roles, deployment environments, alternate WIT worlds, or user-selected capability features.

## Completed

- [x] Added bounded 64 KiB byte-stream batching to the Golem `wit-bindgen` fork. The cap is applied
  before allocating a retained view, covers aliases and WASI filesystem streams, and retains the
  existing non-byte element limit.
- [x] Regenerated the MoonBit SDK from the generator without hand-editing generated bindings.
- [x] Opted the handwritten MoonBit tool stdout adapter into the same 64 KiB byte window. Boundary
  tests cover arrays and byte views, including a 131,073-byte blocked-drain payload.
- [x] Replaced the Rust no-agent proof with automatic linker-visible capability hooks while keeping
  every mandatory agent, tool, middleware, and snapshot export.
- [x] Added automatic same-world minimal export roots for MoonBit, Scala, TypeScript, and Effect.
  Empty, tool-only, agent-only, middleware-only, and mixed fixtures preserve the full ABI while
  bundler/linker DCE removes absent runtimes.
- [x] Specialized generated Rust tool descriptors. Static macro-known descriptors use prepared
  validation and owned WIT conversion without retaining unrelated rich validators; dynamic cases
  continue to use the full path and compile-fail/descriptor parity coverage.
- [x] Made the Rust guest schema dependency graph capability-sensitive while keeping host/full
  validation rich and fail-closed.
- [x] Added shared owned synchronous Rust export-result lowering in `wit-bindgen`, preserving
  offsets, allocation handoff, post-return, async borrowing, resources, and nested stream/future
  behavior.
- [x] Added shared cross-interface MoonBit canonical-ABI lift/lower helpers in `wit-bindgen` and
  regenerated the SDK. Imported interface types are shared without introducing package cycles;
  export-owned types remain local. The filesystem component is 13.0% smaller under the unchanged
  full ABI, and no outlining flag is required.
- [x] Enabled Rust release symbol stripping and added reproducible component size attribution,
  optional restricted `wasm-opt -Oz`, validation, WIT equality checks, and informational CI
  artifacts without thresholds.
- [x] Rebuilt the Rust and MoonBit filesystem components through the integrated CLI and reran direct
  and guest-invoked worker-executor behavior tests.
- [x] Repeated the optimized-host 64 KiB filesystem benchmark after integration. The final results
  are recorded in `filesystem-tools-comparison.md`.

## Verified in the contributing workstreams

- Rust: ten lean/rich native and release cases, six compile-fail fixtures, SDK/macro/schema/tool
  tests, and three real worker streaming/middleware/owner tests.
- MoonBit: full SDK and generator suites, four full-world fixtures, canonical ABI behavior,
  snapshot behavior, byte-window boundaries, and deterministic regeneration.
- Scala: sbt/codegen capability fixtures and ABI checks. The Mill codegen/plugin, empty and
  middleware fixture compile/link matrix also passes with the fixture's middleware guest stub.
- TypeScript: SDK and REPL suites, five capability fixtures, package/lint/type checks, component
  validation, and WIT equality.
- Effect: SDK suite, five capability fixtures, package/contracts/artifact checks, component
  validation, and WIT equality.

## Still external

- The `wit-bindgen` commits are pinned by exact revision but live in the separate
  `golemcloud/wit-bindgen` repository. They must be pushed there before another checkout can resolve
  the pin without a local Git URL rewrite.
- Scala's real server/CLI latency and deployed snapshot/RPC checks were not run in its contributing
  workstream. The final filesystem benchmark covers the Rust and MoonBit built-in components on the
  real worker executor, not Scala, TypeScript, or Effect.
- Restricted Binaryen optimization remains opt-in. No generic component metadata-DCE graph or
  deployment-time optimization was enabled.

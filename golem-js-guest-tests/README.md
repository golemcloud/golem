# In-process JavaScript guest tests

These tests answer a narrow question: can a small application built with the current
TypeScript or Effect SDK be put into the real JavaScript guest, preinitialized, and
discovered? They avoid building or starting the CLI and servers. They do not replace
the CLI integration or worker-executor suites, which still test the complete build
command and worker behavior.

Only the small application sources are committed. The SDK component bundlers turn
them into JavaScript bundles and source maps during test preparation. These outputs
are temporary build inputs, not snapshots to update or commit. Ordinary SDK tests
bundle in memory and never compare bytes against checked-in files. A comment edit,
Git line-ending conversion or a different pnpm store layout cannot cause a fixture
drift failure.

## Prepare and run

Install the repository's Rust and JavaScript toolchains, including Node 24, pnpm,
the pinned `wasm-rquickjs-cli`, the `wasm32-wasip2` Rust target and WASI SDK. Install
both SDKs' dependencies and build their guest templates from this checkout:

```shell
# From sdks/ts:
pnpm install --frozen-lockfile
pnpm --filter @golemcloud/golem-ts-sdk run build
pnpm run build-agent-template

# From sdks/effect:
npm ci
npm run build-agent-template

# From the repository root:
cargo make js-guest-tests
```

`js-guest-tests` first runs `prepare-js-guest-tests`. Preparation removes the old
bundle output directory, rebuilds the TypeScript SDK, and runs the two SDK bundling
cases against current sources. The Effect case loads its SDK sources directly.
Outputs go to `<Cargo target directory>/js-guest-bundles/<sdk>/<fixture>/`.
The task uses cargo-make's resolved target directory, including redirected targets.
It does not build the WASM templates: rebuild those when wrapper, WIT or toolchain
inputs change, following the SDK contributor guides.

Preparation is also available separately:

```shell
cargo make prepare-js-guest-tests
```

For a Rust-only edit, reuse the prepared bundles:

```shell
cargo make --skip-tasks prepare-js-guest-tests js-guest-tests
```

Do not skip preparation after an SDK, component-plugin or fixture-source change.
For a direct `cargo test -p golem-js-guest-tests --test js-guest` run, set
`GOLEM_JS_GUEST_BUNDLE_DIR` to the absolute prepared output directory. The cargo-make
tasks set it automatically. Missing bundles or templates fail with instructions;
the tests never silently skip a guest. The Rust tests start no external processes.

## What the tests cover

The `js-guest` binary exercises both build presets with each freshly bundled
application and its SDK's real `agent_guest.wasm`. The quick path injects the
source into the template without Wizer. The optimized path passes the source to
the unchanged template's initializer through
`golem_common::javascript::preinitialize_js`, the same operation as `golem build`.
The initialization export, shared Wasmtime configuration and copy budget have
one source of truth. That operation is behind the optional `javascript` feature;
other `golem-common` consumers need not depend on the JavaScript optimizer.

The test then calls the guest's discovery export through the production component
metadata extractor. Each application computes an asymmetric arithmetic value and
captures `Date.now()` in its agent description during module initialization.
Discovery must return the expected value and the preinitialization clock of zero.
Without preinitialization, discovery captures the host's current clock instead.
The arithmetic is a recognizable marker, not an arithmetic conformance test.

The compilation-cache tests compare cold, warm and uncached snapshots byte for
byte. A JS edit must report a native compilation cache hit and discover the edited
description. Parallel applications must match their uncached snapshots. Different
engine settings must miss, and truncated entries must recompile successfully.
Sparse cache entries force the cache above the 512 MiB limit and check eviction
to the 80% target, retention of recently used modules and their stats, and protection
of in-progress writes. An unavailable cache must produce the same Wasm as an uncached build.
The cache contains native code, never initialized stores, instances or JS state.
Golem trims oversized caches after builds, in addition to Wasmtime's background
eviction. Parallel builds can temporarily exceed the limit. The cache lives at
`<Golem config directory>/js-compilation-cache`; deleting it only makes the next
build cold. The output Wasm is standalone and does not need this cache or external JS.

The failure tests preinitialize small JavaScript sources, written inline in the Rust
tests, in the TypeScript guest. They check the error that `golem build` prints when
preinitialization fails: a module-level host call names the component and the
qualified import, a module-level throw keeps its JavaScript message and stack, and
a corrupt input keeps the optimizer's original error.

The tests do not call the agent's `get` method or cover worker execution, awaited
I/O or source-map diagnostic accuracy. Source maps accompany the build outputs for
inspection; dependency paths omit pnpm's virtual-store prefix and use forward slashes.
The separate CLI and worker-executor suites retain their existing coverage.

## Add or change an application

- TypeScript source: `sdks/ts/packages/golem-ts-sdk/tests/components/js-guest-smoke.ts`;
  bundling case: `tests/component-bundle.test.mjs` in that package.
- Effect source: `sdks/effect/test/fixtures/capabilities/js-guest-smoke.ts`;
  bundling case: `test/component-build.test.ts` in that SDK.

Use the real component plugin and keep real host imports external, not Vitest host
fakes. Call `writeBundleFixture(sdk, fixture, files)` from an SDK bundling case.
Each fixture owns its own subdirectory; writing one fixture does not remove another.
The preparation task selects cases named `builds JS guest bundle fixtures` in these
two files. Include new guest inputs in those cases (or extend the task's selection)
and consume the new paths from the Rust tests. Run `cargo make js-guest-tests` and
commit only sources and assertions, never the generated bundles or maps.

## CI ownership

Main validation runs `main-validation-js-guests` after TypeScript and Effect SDK
validation, with fresh local templates and the general native producer's binaries
metadata. It runs the same preparation and tests as `cargo make js-guest-tests`.
Bundling or guest-test failure fails preparation and blocks dependent consumers.
The task appears in prepare progress and contributes CTRF results.

There is no separate GitHub Actions job. The ordinary root `unit-tests` task runs
library tests only and does not execute this integration-test target. The task
accepts the existing `TEST_R_BUILD_OPTS` and report options for prebuilt native tests.

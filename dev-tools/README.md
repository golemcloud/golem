# dev-tools

A **separate cargo workspace** for small build/dev tooling, kept out of the main
workspace (so it isn't pulled into `cargo build --workspace`, clippy, udeps, release,
or `set-version`).

## Intentionally std-only

Tools here use **only the Rust standard library — no dependencies, not even for tests.**
This is deliberate: these tools run on the hot path (e.g. `dir-mirror` is invoked by the
`wit` cargo-make task before every build), so they must compile, check, and test almost
instantly. Pulling in a dependency tree (proc-macros, async runtimes, a test framework,
etc.) would add seconds of cold-compile time to the build and to CI for no real benefit
at this size.

Concretely:

- **No runtime/build dependencies** — the `[dependencies]` table stays empty.
- **No dev-dependencies** — tests are plain `#[test]` (libtest) with small std helpers
  (e.g. a tiny temp-dir struct), not a test framework. This differs from the main
  workspace, which uses `test-r`/`cargo-test-r`; that machinery is not worth its
  compile cost here.
- Run the tests with plain `cargo test` (the cargo-make `dev-tools-tests` task), **not**
  `cargo-test-r` — in CI cargo-test-r runs in nextest-archive-reuse mode for the main
  workspace and rejects `--manifest-path` for a separate workspace.

If a tool ever genuinely needs a dependency, reconsider whether it belongs here or in the
main workspace.

## Go bindings generator

`go-bindgen/` is an independent Cargo workspace, excluded from this std-only workspace.
It pins the Go backend and core from `golemcloud/wit-bindgen` at one immutable revision.
The driver enables package-private memory conversion helpers without upgrading
`componentize-go`, the Go compiler or the Go bindings runtime.

Run `cargo make generate-sdk-go-bindings` to regenerate the SDK bindings, apply the
existing fixups, format the output and compile it for WebAssembly. The driver replaces
only the generated output directory; hand-written `internal/exports` stays separate.
Run its checks separately with `--manifest-path dev-tools/go-bindgen/Cargo.toml`:
`cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
The Go CI job runs these checks because `dev-tools-tests` does not cover this workspace.

## Tools

- **`go-bindgen-fixup`** — post-processes the generated Go SDK bindings (`sdks/go/golem/internal/wit`)
  to work around a `wit-bindgen` `crates/go` codegen bug: variant case tag constants are named
  `<Variant><Case>`, which collides with the record type generated from a WIT record named
  `<variant>-<case>` (golem's `queued-card-event`), producing Go that does not compile. Renames
  the colliding *constant* to `<Name>Tag`. Collision detection is generic, not hardcoded. Invoked
  by `cargo make generate-sdk-go-bindings`; **delete once fixed upstream**.
- **`dir-mirror`** — idempotent directory mirror: makes a destination directory a
  byte-identical copy of a source directory, rewriting only changed files (preserving
  mtimes) and pruning stale ones. Used by the `wit` task to sync the per-crate
  `wit/deps` copies without triggering needless rebuilds.

## CI / cargo-make

- `cargo make dev-tools-tests` — runs the tests (plain `cargo test`). In CI this runs
  first in the `build-and-store` job, to fail fast if the build tooling is broken.
- `cargo make check` / `cargo make fix` — also lint this workspace (the
  `*-dev-tools` rustfmt/clippy tasks).

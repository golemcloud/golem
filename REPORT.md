# Build reproducibility report — TypeScript component pipeline

## TL;DR

While wiring the existing build scripts under `test-components/` into a
hermetic environment, the TypeScript pipeline failed two basic
reproducibility properties: a *content-addressable* output (same inputs
→ same bytes) and a *predeclared* dependency set (deps known before the
build runs). The Nix wrapper made these visible, but the underlying
problems are not Nix-specific. They affect every contributor who tries
to verify that their build matches a peer's, every CI cache that keys on
inputs, and every supply-chain consumer who wants to confirm an
artifact came from a particular source tree.

## The two problems

### 1. The dependency set is generated at build time

The agent-guest WebAssembly module — the host shim every TypeScript
component links into — is not a checked-in Rust crate. It is generated
on every build by `wasm-rquickjs generate-wrapper-crate`, which emits a
fresh `Cargo.toml` and `Cargo.lock` based on the WIT input plus
whichever version of `wasm-rquickjs` happens to be on the developer's
PATH.

The downstream consequences:

- Two developers running the same `pnpm run build-agent-template` on the
  same source tree may end up with different `Cargo.lock` files if their
  wasm-rquickjs versions diverge. There is nothing in the repo recording
  what the expected lock should look like.
- Reviewers cannot diff a contributor's lockfile against `main` because
  the lockfile only exists transiently inside `golem-temp/`.
- Any tool that wants to vendor or audit the dependency tree
  (`cargo-deny`, `cargo-audit`, SBOM generators, supply-chain scanners,
  `nix flake lock`) needs the lockfile to exist *before* the build
  starts. Today it does not.

The fix is to commit the generated `agent-template/` (or at minimum its
`Cargo.lock`) and regenerate it on a controlled schedule, the same way
language-binding code generators are typically handled: the generated
artifact is part of the source tree, with a `regenerate.sh` and a CI
check that the committed copy matches what the generator currently
produces.

### 2. Build outputs are not bit-stable across runs

We tested whether the same source tree, built twice in identical
environments, produces the same `.wasm` files. It does not. With
`SOURCE_DATE_EPOCH=1` set, with `RUSTFLAGS=-C codegen-units=1 -C
metadata=stable -C strip=symbols`, with the same toolchain and
dependency cache, three sequential builds of `test-components-ts`
produced three different output hashes:

```
sha256-AVogcRF0s1kdCp5Qi6FN3Q3g6PpgyYq77pZOF0VMsWQ=
sha256-4ou+yBiR0gGSFD1WCXH/RdPzEcKB6E2vAxiZF4DHgr4=
sha256-iqMO1vQDHGpLmzcXiSiTGR/YxnyT4La+Ka1bC7j55sM=
```

The non-determinism comes from somewhere in the chain `pnpm install →
rollup → cargo build (wasm32-wasip2) → wasm-tools → wasm-rquickjs`. We
didn't isolate the exact source — file-iteration order, mtime
preservation in tarball entries, build-id injection, and cargo's
parallel codegen all routinely cause this kind of drift. The point is
not which one is to blame; it's that nothing in the pipeline is
verifying that successive runs converge.

This breaks several useful properties:

- **Content-addressable caching** (CI, sccache, Nix, etc.) cannot key on
  source — it can only key on output, defeating most of the savings.
- **Bisecting a regression** in a generated `.wasm` is hard when "did
  the artifact change" can't be answered by comparing hashes.
- **Supply-chain verification** ("rebuild the release tag and confirm
  the published `.wasm` matches") is not currently possible.
- **Diffing two PRs' built outputs** requires re-running the entire
  pipeline twice and accepting that mismatches may be noise.

## Why we noticed

The test components are gitignored and rebuilt locally. As long as
"works on the contributor's laptop" is the bar, neither of these issues
trips an alarm. The moment we asked "can the project's build be
reproduced from source on a clean machine?", both did.

## Recommended follow-ups

- Commit `sdks/ts/packages/golem-ts-sdk/agent-template/` (Cargo.toml,
  Cargo.lock, src) and add a `make regenerate-agent-template` target
  plus a CI check that the committed copy is in sync with the
  generator's current output.
- Audit one round of `cargo build --target wasm32-wasip2` against the
  Rust reproducibility checklist
  (<https://reproducible-builds.org/docs/build-path/> and adjacent),
  pinning `--remap-path-prefix` and `-Csymbol-mangling-version=v0` so
  build-paths and codegen IDs don't drift.
- Inventory the npm/pnpm `prepare` lifecycle hooks: today, installing a
  workspace `file:` dep re-runs `pnpm build` inside the dep, which
  silently regenerates whatever the parent build just patched. Either
  remove the `prepare` hooks from the published packages or freeze
  their outputs into the repo.
- Add a CI job that builds `test-components-ts` twice in the same job
  and `diff -r`s the output. The day that diff is empty, every other
  reproducibility-dependent feature on this list becomes available.

The Nix flake we landed during this investigation is one consumer of
these properties; the gaps it surfaced are about the project's build
pipeline, not about the wrapper around it.

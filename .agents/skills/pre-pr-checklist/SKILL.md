---
name: pre-pr-checklist
description: "Selects affected local checks before opening or merging a pull request. Use to verify formatting, linting, builds and targeted tests without defaulting to full suites."
---

# Pre-PR Checklist

Validate the smallest dependency and behavior scope that fully covers the change. Follow the repository's local verification policy before opening or merging a PR: main PRs run no tests, and `Main validation` checks the latest main commit after merge. Maintenance branches retain full CI.

## 1. Classify the Change

Inspect all files in the PR, including generated files. A change can belong to more than one row:

| Scope | Minimum local verification |
|---|---|
| Root Rust crate(s) | Package-scoped format check, clippy, check/build, and affected tests |
| Shared Rust API, proc macro, or shared type | `cargo make check-clippy` finds a caller in another crate that no longer compiles; when the behaviour changes, run the existing tests of the callers that exercise that behaviour |
| Shared contract, as `AGENTS.md` defines it under "Before Submitting a PR" | Verify the producer, then smoke-test the callers: the closest existing tests that run the changed path, one for each affected area (worker executor, services, CLI, each SDK that the contract reaches). The usual form is an integration test that builds, deploys and invokes a component. Build the test components that the selected tests need. When no existing test runs the changed path, say so in the PR description |
| `dev-tools/` | Use `--manifest-path dev-tools/Cargo.toml`, except the standalone Go generator uses `--manifest-path dev-tools/go-bindgen/Cargo.toml`; do not check the root workspace unless it is also affected |
| Rust SDK (`sdks/rust/`) | Use that SDK's Cargo workspace checks; do not run root `cargo make fix` or `cargo make build` |
| TypeScript SDK (`sdks/ts/`) | Check affected packages; rebuild the agent template only when its runtime/WIT inputs changed |
| Scala SDK (`sdks/scala/`) | Check affected sbt projects and Scala versions |
| MoonBit SDK (`sdks/moonbit/`) | Check affected MoonBit package(s) and target(s) |
| Docs or Markdown only | Use docs/native formatting, links, or build checks as applicable; no Rust check by default |
| Generated source or schema | Run the owning generator, review generated diffs, and run a local drift check when it supports intentional dirty changes; then verify affected consumers |
| Cross-cutting workspace, WIT, or build-system change | Follow the owning skill's impact table and broaden to all affected subsystems |

Do not treat a file path alone as proof of narrow impact. Public interfaces, feature changes, macros, build scripts, and shared generated inputs can affect consumers outside the edited crate.

## 2. Format and Lint the Affected Scope

For one root-workspace Rust crate:

```shell
cargo fmt -p <crate> -- --check
cargo clippy -p <crate> --all-targets --no-deps -- -D warnings
```

For a separate workspace, add its manifest path. For example:

```shell
cargo fmt --manifest-path dev-tools/Cargo.toml -p <crate> -- --check
cargo clippy --manifest-path dev-tools/Cargo.toml -p <crate> --all-targets --no-deps -- -D warnings
```

Then, for any root-workspace Rust change, lint the whole workspace, as `Main validation` does. Package-scoped clippy does not see a caller in another crate that the change did not update, or a lint that the change raises in another crate:

```shell
cargo make check-clippy
```

Use the current stable toolchain (`rustup update stable`), as `Main validation` does, because the lints change between toolchains. When the check fails in a crate that the change does not touch, run `cargo clippy --all-targets --keep-going -- --no-deps -Dwarnings`, so that cargo still lints the other crates.

Prefer non-mutating checks for final verification. If fixes are needed, use the equivalent scoped `cargo fmt` or `cargo clippy ... --fix` command and inspect the resulting diff.

Do **not** run `cargo make fix` by default. It formats and applies clippy fixes to the entire root workspace and the separate `dev-tools` workspace, can modify unrelated dirty or staged files, and does not check the SDK workspaces.

Use the native SDK commands documented in each SDK's `AGENTS.md`. Formatting and linting are not required for languages or workspaces with no relevant changed files.

## 3. Build the Affected Scope

For a root Rust crate, start with:

```shell
cargo check -p <crate> --all-targets
```

Use `cargo build -p <crate>` when an executable or build artifact is needed. Also check directly affected consumers when changing a public API, shared type, proc macro, feature set, or build script.

Reserve `cargo make build` for changes whose root-workspace impact is broad or cannot be isolated. It builds the entire root Cargo workspace and does not build the SDKs.

## 4. Run the Right Tests

Start with the smallest test that exercises the changed behavior:

```shell
cargo test -p <crate> -- <test_name> --report-time
```

Broaden for a changed shared contract (step 1), for a specific affected contract that narrower checks cannot cover, or for an explicit user request. A changed shared contract gets a smoke test of its callers: the closest existing tests that run the changed path, also when they need built test components. The absence of PR tests does not justify full suites:

| Impact | Broader command when needed |
|---|---|
| Broad core logic, shared types, or utilities | `cargo make unit-tests` |
| Broad worker executor behavior | Relevant worker test group, then `cargo make worker-executor-tests` only if needed |
| Broad service and CLI integration | Relevant tagged integration tests, then `cargo make integration-tests` only if both broad suites are warranted |
| Broad CLI behavior | Targeted `cargo-test-r` filters, then `cargo make cli-integration-tests` only if needed |
| HTTP or gRPC endpoint behavior | Relevant endpoint tests; use the full HTTP/gRPC API suite for cross-cutting endpoint changes |
| SDK behavior used by the platform | Relevant SDK tests plus only the platform tests that exercise the changed integration |
| Go SDK (`sdks/go/`) | the per-module build, test and lint set in the `golem-go-development` skill; changes that reach a running component also rebuild `test-components/agent-sdk-go` and run the `agent_sdk_go` executor tests |
| CLI structured output/schema | `cargo test -p golem-cli cli_output_schema_ --lib` + `cargo make check-cli-output-schema` |
| CLI JSON output affecting skill tests | Update and run affected skill harness tests/scenarios; run harness build/tests only if harness code or tests changed |

Never use `cargo make test` as a default. Whenever a test is modified, run that test. Build only the test components needed by the selected worker, integration, or CLI tests.

## 5. Regenerate Required Artifacts

These are hard requirements when their inputs change:

| Input changed | Required action |
|---|---|
| HTTP API endpoints/schema | `cargo make generate-openapi`; verify affected service and `golem-client` consumers |
| Service config structs/defaults | `cargo make generate-configs` |
| Root WIT interfaces | `cargo make wit`, review synchronized diffs, and follow the WIT impact table; `cargo make check-wit` is a clean-checkout/CI check |
| TS SDK runtime bundle/WIT inputs | Build `@golemcloud/golem-ts-sdk`, then run `npx pnpm run build-agent-template` in `sdks/ts/` |
| Scala guest runtime WIT/input | Regenerate `agent_guest.wasm` using the Scala SDK workflow |
| Skill catalog (`golem-skills/skills/**`) | `cargo make generate-docs-skills` |
| Go SDK WIT bindings (after a WIT change) | `cargo make generate-sdk-go-bindings` |
| CLI output schema summary | `cargo make update-cli-output-schema-summary` |

Review generated diffs and commit them with their source changes. Do not replace an owning generator with a hand edit.

## 6. Review the Final Diff

Only stage files directly related to your change:

```shell
git diff --stat          # Review unstaged changes
git add <specific-files> # Stage only relevant files
```

**Never use `git add -A` or `git add .`** — they may include unrelated changes from concurrent work.

## Checklist

1. [ ] Every changed area is classified, including generated files and consumers
2. [ ] Affected code is formatted and linted with native, scope-appropriate checks, and `cargo make check-clippy` passes for a root-workspace Rust change
3. [ ] Affected packages build or type-check
4. [ ] Modified and behaviorally affected tests pass
5. [ ] Required artifacts are regenerated and their drift checks pass
6. [ ] Each changed shared contract has a smoke test of its callers, and the PR description names the selected tests and each one that could not run
7. [ ] Each other broader check covers an identified affected contract that narrower checks cannot cover
8. [ ] Only relevant files are included in the PR

In the completion report, name the checks run, their outcomes and remaining coverage limits. Distinguish local verification from post-merge Main validation; do not imply PR tests will cover omitted checks.

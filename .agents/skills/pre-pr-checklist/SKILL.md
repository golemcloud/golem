---
name: pre-pr-checklist
description: "Final checks before submitting a pull request. Use when preparing to create a PR, to ensure formatting, linting, and the correct tests have been run."
---

# Pre-PR Checklist

Run through this checklist before creating a pull request. It ensures code quality gates pass and the right tests have been executed.

## Step 1: Run cargo make fix

**Always required.** This runs `rustfmt` and `clippy` with automatic fixes:

```shell
cargo make fix
```

Address any remaining warnings or errors that couldn't be auto-fixed.

## Step 2: Run the Right Tests

Choose tests based on what you changed. **Do not run `cargo make test`** — it runs everything and takes a very long time.

| What Changed | Test Command |
|---|---|
| Core logic, shared types, utilities | `cargo make unit-tests` |
| Worker executor functionality | `cargo make worker-executor-tests` |
| Service integration | `cargo make integration-tests` |
| CLI changes | `cargo make cli-tests` |
| HTTP API endpoints | `cargo make api-tests-http` |
| gRPC API endpoints | `cargo make api-tests-grpc` |
| Rust SDK (`sdks/rust/`) | `cargo test -p golem-rust` + `cargo make worker-executor-tests` |
| TypeScript SDK (`sdks/ts/`) | `npx pnpm run test` (in `sdks/ts/`) + `cargo make cli-tests` |

If your change spans multiple areas, run multiple test suites.

### Worker executor test groups

For faster iteration, worker executor tests can be run by group:

```shell
cargo make worker-executor-tests-group1
cargo make worker-executor-tests-group2
cargo make worker-executor-tests-misc
```

## Step 3: Regenerate Artifacts (if applicable)

| What Changed | Regeneration Command |
|---|---|
| HTTP API endpoints | `cargo make generate-openapi` then `cargo clean -p golem-client && cargo build -p golem-client` |
| Service config structs/defaults | `cargo make generate-configs` |
| WIT interfaces | `cargo make wit` |
| TS SDK runtime code | `npx pnpm run build-agent-template` (in `sdks/ts/`) |

## Step 4: Verify Build

```shell
cargo make build
```

## Step 5: Review Staged Files

Only stage files directly related to your change:

```shell
git diff --stat          # Review unstaged changes
git add <specific-files> # Stage only relevant files
```

**Never use `git add -A` or `git add .`** — they may include unrelated changes from concurrent work.

## Quick Reference

Minimum steps for any PR:

```shell
cargo make fix           # Format + lint
cargo make build         # Full build
# Run appropriate tests from the table above
```

## Checklist

1. [ ] `cargo make fix` run — no remaining warnings
2. [ ] Correct test suite(s) run — all pass
3. [ ] Artifacts regenerated if applicable (OpenAPI, configs, WIT, agent template)
4. [ ] `cargo make build` succeeds
5. [ ] Only relevant files staged for commit

---
name: verifying-merged-branches
description: "Checks a merge or rebase of main into a branch (or a branch onto main) for reverted changes and builds every project the merge touched, including the Rust SDK, test components and SDK checks that the root workspace build does not cover. Use after resolving merge or rebase conflicts, before pushing a merged branch, or when a merged branch fails Main validation."
---

# Verifying Merged Branches

`cargo check --workspace` covers only the root Cargo workspace. A merge also changes code that
only other builds compile: the `sdks/*` workspaces, the test components (built by
`test-components/build-components.sh` through `golem-cli`), CLI test fixtures and the Scala REPL
samples. PRs to `main` run no builds, so a bad resolution there shows up only in `Main validation`
after it lands.

## 1. Resolve conflicts without a blanket strategy

- Do not resolve source conflicts with `git checkout --ours/--theirs`, `-X ours/theirs` or
  `git merge-file --ours/--theirs` over many files. Each of these keeps one side of every
  conflicting hunk. When the other side carries an API migration, the hunk silently brings the
  old API back.
- For each conflicting hunk, find out what each side changed (`git log -p <merge-base>..<side> --
  <path>`). When one side migrated an API, start from the migrated side and re-apply the other
  side's change to it.
- Take one side whole only for generated files, then rerun the owning generator
  (`cargo make wit`, `cargo make generate-openapi`, `cargo make generate-sdk-go-bindings`, the
  SDK `generate-dts` scripts, MoonBit codegen).
- Search-and-replace migrations (for example `get` to `upsert`) can hit unrelated names, such as
  an agent method called `get`. Review every replaced line.

## 2. Find reverts and affected projects

Run the script from the repository root after the merge commit exists:

```shell
python3 .agents/skills/verifying-merged-branches/scripts/merge_impact.py
```

The defaults compare `HEAD^1` (the branch before the merge), `HEAD^2` (the incoming tip) and
`HEAD`. Other cases:

| Case | Arguments |
|---|---|
| Resolution not committed yet | `--ours HEAD --theirs MERGE_HEAD --worktree` |
| Rebase | `--ours ORIG_HEAD --theirs <upstream> --result HEAD` |
| An older merge commit `M` | `--ours M^1 --theirs M^2 --result M` |

Read the output in this order:

1. **Removed lines that are back.** One side deleted these lines and the result still has them.
   This is how a resolution that kept the wrong side shows up. Open every file in this section and
   either fix it or confirm the line is meant to stay. `--samples N` prints more lines per file.
2. **Added lines that are missing.** One side added these lines and the result lacks them.
   Legitimate rewrites also land here, so check the files whose change the result should keep.
3. **Checks to run.** Each check comes with the files that require it. A changed SDK also selects
   the test-component group written against that SDK.
4. **Needs a manual decision.** Paths the script cannot map to a build, such as CLI integration
   test fixtures, Scala REPL samples and test components outside any group.

## 3. Run every selected check

Prepare the inputs first, because stale local artifacts produce false failures:

- Build a fresh CLI: `cargo build -p golem-cli`. Set `GOLEM_CLI=$PWD/target/debug/golem-cli`
  for `build-components.sh`. A CLI built before the merge can generate stale bindings.
- Prepare each SDK that a selected component group uses, with `WASI_SDK_PATH` set (for example
  `/opt/wasi-sdk`): `cargo make prepare-sdk-ts`, `prepare-sdk-effect`, `prepare-sdk-scala`,
  `prepare-sdk-moonbit`.
- Test components that depend on an SDK through `file:` can hold a copied package in
  `node_modules`. When the build reports exports missing from the SDK source, reinstall that
  package (`rm -rf node_modules/@golemcloud/<pkg> && npm install`).
- For quick TS and Effect builds, set `GOLEM_TS_PRESET=quick`.
- Check free disk space before long builds. The `target/` directories of the separate workspaces
  and test components are rebuildable caches.

Then run the checks the script printed, for example:

```shell
cargo make check-clippy
cargo make validate-sdk-rust-check
cargo make test-component-middleware-unit-tests
GOLEM_CLI=$PWD/target/debug/golem-cli GOLEM_TS_PRESET=quick ./test-components/build-components.sh ts
```

To build one test-component app rather than its group, run the CLI in the app directory, as its
`AGENTS.md` describes. For example, `tool-streaming-ts`:

```shell
GOLEM_TS_PACKAGES_PATH="$(cd ../../sdks/ts/packages && pwd)" ../../target/debug/golem-cli --preset release build --yes --skip-check
```

Builds can migrate files in a test component's source directory. Keep real migrations. Revert
changes that a local toolchain mismatch caused, such as MoonBit formatting of generated files by a
different `moon` version.

## 4. Report the result

Before pushing, report:

- the files from "Removed lines that are back", and what you did with each
- every selected check and its result
- every check you could not run, and why (missing toolchain, needs a server or Docker). These are
  coverage gaps, not passes.

Run the `pre-pr-checklist` skill for the change's own tests. This skill covers what the merge
itself can break.

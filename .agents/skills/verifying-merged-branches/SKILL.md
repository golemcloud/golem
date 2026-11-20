---
name: verifying-merged-branches
description: "Checks a merge or rebase of main into a branch (or a branch onto main) for reverted changes and builds every project the merge touched, including the SDK workspaces, test components and fixtures that the root workspace build does not cover. Use after resolving merge or rebase conflicts, before pushing a merged branch, or when a merged branch fails Main validation."
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
- For each conflicting hunk, find out what each side changed
  (`git log -p <merge-base>..<side> -- <path>`). When one side migrated an API, start from the
  migrated side and re-apply the other side's change to it.
- Take one side whole only for generated files, then rerun the owning generator
  (`cargo make wit`, `cargo make generate-openapi`, `cargo make generate-sdk-go-bindings`, the
  SDK `generate-dts` scripts, MoonBit codegen).
- Search-and-replace migrations (for example `get` to `upsert`) can hit unrelated names, such as
  an agent method called `get`. Review every replaced line.

## 2. Look for reverted changes

Name the three commits:

| Case | `OURS` | `THEIRS` | `RESULT` |
|---|---|---|---|
| Merge commit `M` (default `HEAD`) | `M^1` | `M^2` | `M` |
| Rebase | `ORIG_HEAD` | the upstream, for example `origin/main` | `HEAD` |

For every file that both sides changed, list the lines that one side removed and the result still
has. This is how a resolution that kept the wrong side shows up:

```shell
base=$(git merge-base "$OURS" "$THEIRS")
comm -12 <(git diff --name-only "$base" "$OURS" | sort) \
         <(git diff --name-only "$base" "$THEIRS" | sort) |
while read -r f; do
  for side in "$THEIRS" "$OURS"; do
    n=$(comm -23 \
          <(git diff -U0 "$base" "$side" -- "$f" | grep '^-[^-]' | cut -c2- | sort -u) \
          <(git diff -U0 "$base" "$RESULT" -- "$f" | grep '^-[^-]' | cut -c2- | sort -u) |
        grep -c '[A-Za-z]')
    [ "$n" -gt 0 ] && echo "$f: $n line(s) that $side removed are still in the result"
  done
done
```

Open every reported file and diff it against the side that removed the lines
(`git diff "$THEIRS" "$RESULT" -- <file>`). Fix the resolution, or confirm that the line is meant
to stay. A revert that the resolution wrote as new code does not show up here; the builds in step 3
catch it.

## 3. Build every project the merge touched

List the touched files with `git diff --name-only "$OURS" "$RESULT"`, plus the files that both
sides changed. Group them by the build that owns them, and run each build:

| Touched path | Build |
|---|---|
| A root workspace crate, `Cargo.toml`, `Cargo.lock` | `cargo make check-clippy` |
| `sdks/<language>/` | `cargo make validate-sdk-<language>` (tasks in `Makefile.sdk-validation.toml`) |
| `test-components/<app>/` | the `build-components.sh` group that lists the app; some Rust crates also have unit tests in `test-component-middleware-unit-tests` |
| `wit/`, `openapi/`, `golem-skills/skills/` | `cargo make wit` and `check-wit`, `check-openapi`, `check-docs-skills` |
| CLI test fixtures under `cli/golem-cli/tests/` | the CLI integration test that uses them (see the `testing` skill) |
| `sdks/scala/test-agents/samples/` | the Scala integration suite (see `golem-scala-integration-tests`) |

When a merge changes an SDK, also build the test-component group for that language, even if no
component file changed. Components break when the SDK API changes under them. A path that no row
covers still needs a build; find the one that compiles it before you skip it.

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

To build one test-component app instead of its group, run the CLI in the app directory, as the
app's `AGENTS.md` describes.

Builds can migrate files in a test component's source directory. Keep real migrations. Revert
changes that a local toolchain mismatch caused, such as MoonBit formatting of generated files by a
different `moon` version.

## 4. Report the result

Before pushing, report:

- the files from step 2, and what you did with each
- every build from step 3 and its result
- every build you could not run, and why (missing toolchain, needs a server or Docker). These are
  coverage gaps, not passes.

Run the `pre-pr-checklist` skill for the change's own tests. This skill covers what the merge
itself can break.

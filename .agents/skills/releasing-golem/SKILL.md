---
name: releasing-golem
description: Prepares and releases Golem with published shared crates, six SDKs, protected built-in publication, and human draft review. Use when planning, running, or recovering a Golem release.
---

# Releasing Golem

Release shared crates, then six SDKs, then prepare Golem on its exact tag. Read
[CONTRIBUTING.md](../../../CONTRIBUTING.md#release-process) and
[BUILTIN_ARTIFACTS.md](../../../BUILTIN_ARTIFACTS.md#tagged-release-preparation).

## Authorization and setup

Do not push tags, dispatch workflows, publish releases, or change settings without
explicit authorization for those actions. Draft publication is a separate human
decision; preparation never publishes the Golem draft. Commands below are a
runbook, not authorization. Substitute approved versions and commit IDs.
Release authorization does not authorize direct pushes or merges to the release
target branch.

Run `cargo make check-release-start` from clean committed source first. This
read-only check verifies GitHub authentication and the protected environment's
App configuration names. It does not validate secret values or registry access.

At release start, select `main` or the matching `<major>.<minor>.x` maintenance
branch. Record its public commit and create a preparation branch from it. Keep
that baseline throughout the release. Commit release fixes and version pins on
the preparation branch; do not merge or rebase later development into it.
Development on `main` can continue independently. Create the maintenance branch
before breaking development starts, and land tooling changes on both active lines.

An administrator must configure GitHub Environment `golem-release` to allow only
release tags. Required reviewers are recommended. Set its variable
`RELEASE_APP_ID` and secret `RELEASE_APP_PRIVATE_KEY` for a GitHub App installed
on `golemcloud/golem` and `golemcloud/golem-builtins` with contents read/write.
Only the protected publication job mints a token scoped to those repositories.
Normal CI must not receive a built-in write token. Do not print credentials.

## 1. Publish the shared snapshot

Use full history and tags; unshallow a shallow checkout before history checks.
Start with clean committed source and choose an unused shared version. The
publication workflow gates uploads on the release-tooling checks and the three
shared-library test suites. Full platform CI is not a prerequisite for this step;
do not wait for unrelated worker, service, or CLI integration tests. The Golem
preparation step runs the full preflight later. With authorization:

```shell
git tag golem-shared-v1.6.0 <approved-shared-commit>
git push origin golem-shared-v1.6.0
gh release create golem-shared-v1.6.0 --repo golemcloud/golem --title 'Shared crates 1.6.0' --generate-notes --latest=false
```

The GitHub release-published event, not the tag push, starts
`publish-golem-shared.yaml`. Its version comes from the tag. Wait for successful
publication of `golem-schema-derive`, `golem-schema`, then `golem-tool-metadata`.
For an RC, add `--prerelease` to the release command. Do not mark the shared
release as the latest Golem release.
Consumers select the nearest `golem-shared-v*` tag on first-parent history, not by
version sorting. Missing tags or multiple shared tags on the selected commit fail.
A tag reachable only through a merged side branch is not a first-parent tag.
There is no `.github/shared-crates-version` file.

After publication, freeze `Cargo.toml`, `Cargo.lock`, `.cargo`, `LICENSE`, and the
three shared crate directories through the SDK and Golem releases. Snapshot
matching and registry checksum/source-commit provenance checks remain required:

```shell
cargo make verify-shared-crates
```

A changed snapshot needs a new shared version and a Rust SDK release against it.
Never overwrite registry versions. Keep tags immutable except for the narrowly
authorized failed pipeline-test recovery described below.

## 2. Publish and pin all six SDKs

Use [sdk-development](../sdk-development/SKILL.md) for Rust, TypeScript, Effect,
and MoonBit build context, [golem-scala-development](../golem-scala-development/SKILL.md)
for Scala, and [golem-go-development](../golem-go-development/SKILL.md) for Go.
Check the corresponding SDK workflow before tagging. With authorization, create
and push each tag at its approved commit:

```shell
git tag golem-rust-v1.6.0 <approved-rust-commit>
git tag golem-ts-v1.6.0 <approved-ts-commit>
git tag golem-effect-v1.6.0 <approved-effect-commit>
git tag golem-scala-v1.6.0 <approved-scala-commit>
git tag golem-moonbit-v1.6.0 <approved-moonbit-commit>
git push origin golem-rust-v1.6.0
git push origin golem-ts-v1.6.0
git push origin golem-effect-v1.6.0
git push origin golem-scala-v1.6.0
git push origin golem-moonbit-v1.6.0
```

Push these tags separately and confirm that each publication workflow starts.
GitHub does not create tag-push events when more than three tags are pushed at
once. Do not combine these pushes with the Go module tags or use `git push --tags`.

SDK versions need not equal the Golem or shared version. These five SDKs publish
on tag pushes. Go releases through module tags, with no publish workflow. First
commit real release versions in Go module requirements, keeping local replacements
for in-tree builds; then tag all three modules at the same approved commit:

```shell
git tag sdks/go/core/v1.6.0 <approved-go-commit>
git tag sdks/go/golem/v1.6.0 <approved-go-commit>
git tag sdks/go/bridge/v1.6.0 <approved-go-commit>
git push origin sdks/go/core/v1.6.0 sdks/go/golem/v1.6.0 sdks/go/bridge/v1.6.0
```

Wait for registry availability. Commit all six SDK pins in
`cli/golem-cli/src/versions.rs`. Before the Golem tag, bump changed exported
built-in tool versions in source, registry provisioning, and defaults. Artifact
versions are separate: all eight use the Golem version without its leading `v`.

## 3. Tag and prepare Golem

The source must be clean, the exact public tag must match the checked source, and
its commit must be the exact head of a PR merged into `golemcloud/golem`'s `main`
or the maintenance branch matching the tag (`1.6.x` for `v1.6.2`).
The PR's merge commit must remain reachable from that public target branch.
Validation uses the GitHub API and requires an authenticated `gh` session with
pull requests read access. In CI, `GITHUB_TOKEN` supplies that read access
independently of the contents-only publication token in `GH_TOKEN`.
A PR into a different maintenance line does not qualify. A shared tag on another
history path does not fix snapshot selection.
The preparation workflow must be available on the default branch for dispatch and
present at the selected tag. Do not dispatch on a branch or pass a separate tag input.
Use a reviewed PR to merge the preparation branch into the selected target
without importing newer development into the release source. Use the repository's
normal squash merge. Record the PR's exact head before merging, then tag that
preparation-branch commit, not the squash commit on the target. The tag preserves
the frozen source and its first-parent shared snapshot; validation proves that
this exact source was merged.
Obtain explicit approval before merging. With authorization:

```shell
git tag v1.6.0 <approved-golem-commit>
git push origin v1.6.0
gh workflow run prepare-golem-release.yaml --repo golemcloud/golem --ref v1.6.0
gh run list --repo golemcloud/golem --workflow prepare-golem-release.yaml
gh run watch <run-id> --repo golemcloud/golem --exit-status
```

`.github/workflows/prepare-golem-release.yaml` validates source and tag, then runs
full reusable CI preflight. It checks published shared crates and all six published
SDKs, without local SDK overrides, including streaming template walkthroughs.
Only after preflight does the protected `golem-release` job create or resume the
draft, rebuild and package all eight inventory entries before any built-in upload,
then publish them and upload the verified `builtin-artifacts.release.json`.
The existing native matrix then builds, signs, notarizes, and attaches all ten
Golem and CLI binaries to the draft. Wait for the entire preparation to succeed.
The publisher guard permits only this workflow's
`workflow_dispatch` on the matching Golem tag in `golemcloud/golem`.

## 4. Human review and final publication

```shell
gh release view v1.6.0 --repo golemcloud/golem
gh release view v1.6.0 --repo golemcloud/golem --web
```

Review the successful preparation, native binaries, release notes, draft state, all eight public
built-in asset sets, provenance, licenses, applicable SBOMs, checksums, and the
uploaded manifest. Follow [publishing-builtin-artifacts](../publishing-builtin-artifacts/SKILL.md)
for packaging details. Do not commit the generated manifest. After explicit
approval, publish the reviewed draft without promoting Latest:

```shell
gh release edit v1.6.0 --repo golemcloud/golem --draft=false --latest=false
```

In the GitHub UI, uncheck **Set as latest release** before publishing. Do not
rely on the draft's initial flag. Only the publication workflow must promote
Latest; a manual promotion can race its channel selection.
The publication event starts `publish-golem-release.yaml`,
which checks successful preparation of the exact tag before running the existing
Docker jobs. It does not repeat full CI or native builds. Preparation itself never
publishes the Golem draft. Maintenance branches also run push CI.

New drafts disable the Latest designation. After a human publishes a draft, the
publication workflow selects GitHub Latest and Docker `:latest` by the highest
published stable Golem version, not branch ancestry. npm uses each package's
published versions: prereleases use `next`, older stable versions use
`release-<major>.<minor>`, and the highest stable version uses `latest`.
Golem 1.6 patches remain the default while Golem 2.0 is prerelease-only, but cannot
replace stable Golem 2.0. SDK versions are independent of Golem versions; npm
compares each package's own versions. Lookup failures stop publication.
Publication workflows serialize uploads across release lines. GitHub Actions
retains only one pending run per concurrency group; let a queued publication
start before triggering another.

## Recovery

Inspect the run and public assets before retrying any action with an uncertain
outcome. Fix infrastructure or permissions without changing tagged source, then
rerun the same preparation run:

```shell
gh run view <run-id> --repo golemcloud/golem --log-failed
gh run rerun <run-id> --repo golemcloud/golem
```

Partial publication is not transactional. Reruns verify existing packages and
assets, publish only missing ones, and verify the complete public set. An existing
manifest must be identical and is reused. A mismatch stops publication; never
delete or overwrite assets, use `--clobber`, or move tags. Changed source or bytes
requires a new release tag, except for the pipeline-test case below. A failed
shared publication can likewise rerun at its same tag after checking registry
state and provenance.

During an explicitly identified pipeline test, a failed shared tag and GitHub
release can be recreated at a fixed commit only with explicit approval for that
deletion and recreation. First verify that all three crate versions are absent
from crates.io, the release has no assets, and no SDK or Golem release consumes
the failed shared tag. General release authorization is not enough. Once any
crate is published, preserve the tag and use normal immutable recovery.

If a bulk tag push created SDK tags but no publication runs, verify the missing
runs before retrying. Pushing an existing tag again does not create an event.
With explicit approval to delete and recreate those tags, recreate each at its
original commit and push it separately. Keep all source commits and versions
unchanged; do not delete the Go tags, which need no publication workflow.

For explicitly authorized local recovery, use Python 3.14 as `python3`, the exact
clean tag, full history, and authenticated write access to both repositories:

```shell
BUILTIN_VERSION=1.6.0 cargo make validate-builtin-release
BUILTIN_VERSION=1.6.0 BUILTIN_DRY_RUN=1 cargo make publish-builtin-artifacts
# External writes: requires separate publication authorization.
BUILTIN_VERSION=1.6.0 cargo make publish-builtin-artifacts
```

Validation fetches refs but does not build; dry run builds/packages without
publication or manifest creation. Local recovery keeps all immutability and
provenance checks. Never bypass the CI guard or give normal CI write credentials.
Builds on different hosts can produce different WASM bytes from the same source.
Do not replace a package when a rebuild differs. To retain a complete package
already verified by the original publisher and publish only missing components,
follow [recovery across build hosts](../../../BUILTIN_ARTIFACTS.md#recovery-across-build-hosts).
This manual procedure requires explicit publication authorization and the original
verification evidence; it does not relax the all-entry publisher's byte checks.

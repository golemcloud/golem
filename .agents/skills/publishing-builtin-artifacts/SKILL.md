---
name: publishing-builtin-artifacts
description: Publishes immutable built-in artifacts during protected tagged Golem release preparation or authorized local recovery. Use for golem-builtins releases, release manifests, checksums, provenance, licenses, or artifact versions.
---

# Publishing Built-in Artifacts

Read `BUILTIN_ARTIFACTS.md` for the release contract and recovery procedure.
Source and build logic stay in `golem`; `golemcloud/golem-builtins` holds only
immutable release assets. Never commit generated WASMs or embed them in published
release binaries. Local install tasks opt into compressed embedding; source
overrides still take precedence. Release mode rejects the embedding feature.

Development and main CI build current sources with
`cargo make ensure-builtin-artifacts`. Do not publish development artifacts or
generate development tool versions. Exported tool versions differ from artifact
versions. Bump changed tools in source, registry provisioning, and defaults during
release preparation. Replacing local bytes does not rebind published tool
coordinates in an existing registry database.

## Protected preparation

Use [releasing-golem](../releasing-golem/SKILL.md) for the complete release order
and authorization gates. The normal publisher is the protected `golem-release`
job in `prepare-golem-release.yaml`, dispatched on the exact Golem tag. It runs
after one full exact-tag CI preparation, not an arbitrary earlier green PR run.
Pushes to `release/*` do not start automatic CI; PRs still do. Read-only jobs
package and validate all eight entries, including Python sources, before any
publication. Preserve the complete Actions bundle and small runtime manifest.
Use the local manifest to compile ten native binaries and four Docker payload
tars. Sign Apple binaries and wait for notarization status `Accepted` before
bundle upload.

The protected job only collects and verifies exact artifact IDs and digests.
Compare all existing destinations before the first write. Publish only missing
built-in packages and upload the ten native binaries and runtime manifest to the
draft. Do not build, package, or execute downloaded code after obtaining the write
token. Upload an attempt-qualified Actions readiness receipt bound to source,
full ref, run, attempt, release ID, and exact artifacts and hashes.
Preparation never publishes the Golem draft.
Only this job mints the GitHub App token from environment variable
`RELEASE_APP_ID` and secret `RELEASE_APP_PRIVATE_KEY`. The App needs contents
read/write on `golemcloud/golem` and `golemcloud/golem-builtins`. Restrict the
environment to release tags; required reviewers are recommended. Normal CI has
no built-in write token. The publisher's CI guard accepts only this workflow's
`workflow_dispatch` on the matching Golem tag in `golemcloud/golem`.

## Authorized local recovery

Use Python 3.14 as `python3` for source packaging; do not rely on the runner's
system Python. Packaging belongs in the read-only job, not the protected publisher.

1. Use a clean exact release-tag checkout with full history. Push the tag and
   create a draft Golem GitHub release first. The tag must identify the exact
   head of a PR merged into public `main`, not its squash commit. Use an
   authenticated `gh` session with pull requests read access for source validation.
2. Run `BUILTIN_VERSION=<version> cargo make validate-builtin-release` for
   source validation. It still requires a draft and clean exact-tag source.
   Run `BUILTIN_VERSION=<version> cargo make package-builtin-artifacts` to build
   and validate all eight packages, including Python sources, without public
   writes. It creates `target/builtin-artifact-release/bundle` with the local
   manifest and package metadata. Add `BUILTIN_DRY_RUN=1` to
   `cargo make publish-builtin-artifacts` to review without public writes; dry run
   preserves the bundle and manifest. Review exports, provenance, licenses, SBOM,
   hashes, and URLs. Inspect an incomplete bundle rather than blindly reusing it.
   Never rebuild over a completed bundle.
3. Explicit invocation of `BUILTIN_VERSION=<version> cargo make publish-builtin-artifacts`
   approves publication of all eight inventory entries. Do not run it unless
   publication is requested. It uses an existing complete bundle rather than
   rebuilding it. If no bundle exists, package all entries before any write.
   All use the tag version without its leading `v`.
4. Compare all existing destinations before the first write. Publish only missing
   assets, verify public bytes, and upload `builtin-artifacts.release.json` without
   replacement. Do not commit the generated manifest. Local built-in recovery does
   not replace native, Docker, or tagged readiness evidence.

Before tags, run `BUILTIN_VERSION=1.6.0 python3 scripts/golem-release.py check-start`.
This fast read-only check covers prerequisites, environment settings, and secret
names. It does not validate the actual private key or guarantee registry readiness.
Before a human publishes the draft, review its assets and run:

```shell
BUILTIN_VERSION=1.6.0 python3 scripts/golem-release.py readiness --tag v1.6.0 --directory target/release-review
```

This command is read-only. After human publication, `publish-golem-release.yaml`
validates that successful tagged preparation completed before publication and
promotes only saved Docker payloads. It runs no full CI or native compilation.
Actions artifacts have 30-day retention. Review and publish within that period
or stop. Missing, expired, or ambiguous evidence fails closed; never automatically
rebuild. For normal partial consumer reruns, rerun failed jobs, not all jobs, with
the saved producer IDs and exact bytes.

The existing `cargo make publish-builtin-artifact` remains a manual packaging
entry point with `BUILTIN_COMPONENT`, `BUILTIN_VERSION`, `BUILTIN_SKIP_BUILD=1`,
and `BUILTIN_DRY_RUN=1` for packaging inspection. Preserve provenance, licenses,
source archives, and applicable SPDX SBOMs. Never overwrite tags or assets.
On partial failure, rerun from the same source. The publisher verifies existing
assets, uploads only missing assets, and verifies the complete public set before
the outer task creates the manifest. A conflicting asset stops publication.
An existing manifest must be identical and is verified and reused. Never use
`--clobber`, move a tag, or bypass the CI guard. Changed bytes require a new tag.

Cross-host rebuilds need not produce identical WASM bytes. For explicitly
authorized recovery that retains complete packages already verified by the
original publisher and publishes only missing components, follow
[recovery across build hosts](../../../BUILTIN_ARTIFACTS.md#recovery-across-build-hosts).
Require the original verification evidence and verify every public asset before
constructing the manifest. This remains exceptional manual recovery, not automated
source reproducibility. Do not bypass a byte mismatch in the all-entry task.

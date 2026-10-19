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
after full reusable CI preflight, creates or resumes the draft, publishes all eight
entries, and uploads the verified manifest. It never publishes the Golem draft.
Only this job mints the GitHub App token from environment variable
`RELEASE_APP_ID` and secret `RELEASE_APP_PRIVATE_KEY`. The App needs contents
read/write on `golemcloud/golem` and `golemcloud/golem-builtins`. Restrict the
environment to release tags; required reviewers are recommended. Normal CI has
no built-in write token. The publisher's CI guard accepts only this workflow's
`workflow_dispatch` on the matching Golem tag in `golemcloud/golem`.

## Authorized local recovery

1. Use a clean exact release-tag checkout with full history. Push the tag and
   create a draft Golem GitHub release first. The tag must identify the exact
   head of a PR merged into public `main`, not its squash commit. Use an
   authenticated `gh` session with pull requests read access for source validation.
2. Run `BUILTIN_VERSION=<version> cargo make validate-builtin-release` for
   source validation. Add `BUILTIN_DRY_RUN=1` to `cargo make publish-builtin-artifacts`
   to build and review packaging without writes to GitHub. Review exports,
   provenance, licenses, SBOM, hashes, and URLs.
3. Explicit invocation of `BUILTIN_VERSION=<version> cargo make publish-builtin-artifacts`
   approves publication of all eight inventory entries. Do not run it unless
   publication is requested. All use the tag version without its leading `v`.
4. The task verifies public bytes, generates
   `builtin-artifacts.release.json`, and uploads it to the Golem release without
   replacement. Publish the draft only after the upload. The publication event
   starts release CI, which fetches the manifest and sets `GOLEM_BUILTIN_RELEASE=1`
   before compilation. Do not commit the generated manifest.

The existing `cargo make publish-builtin-artifact` remains a manual packaging
entry point with `BUILTIN_COMPONENT`, `BUILTIN_VERSION`, `BUILTIN_SKIP_BUILD=1`,
and `BUILTIN_DRY_RUN=1` for packaging inspection. Preserve provenance, licenses,
source archives, and applicable SPDX SBOMs. Never overwrite tags or assets.
On partial failure, rerun from the same source. The publisher verifies existing
assets, uploads only missing assets, and verifies the complete public set before
the outer task creates the manifest. A conflicting asset stops publication.
An existing manifest must be identical and is verified and reused. Never use
`--clobber`, move a tag, or bypass the CI guard. Changed bytes require a new tag.

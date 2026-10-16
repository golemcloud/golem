---
name: publishing-builtin-artifacts
description: Publishes immutable built-in artifacts during manual tagged Golem release preparation. Use for golem-builtins releases, release manifests, checksums, provenance, licenses, or artifact versions.
---

# Publishing Built-in Artifacts

Read `BUILTIN_ARTIFACTS.md` for the release contract and recovery procedure.
Source and build logic stay in `golem`; `golemcloud/golem-builtins` holds only
immutable release assets. Never commit generated WASMs or embed them in binaries.

Development and main CI build current sources with
`cargo make ensure-builtin-artifacts`. Do not publish development artifacts or
generate development tool versions. Exported tool versions differ from artifact
versions. Bump changed tools in source, registry provisioning, and defaults during
release preparation. This workflow does not require redeployment or database reset.

## Manual release

1. Use a clean exact release-tag checkout with full history. Push the tag and
   create a draft Golem GitHub release first. The tag must originate on main.
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
Never publish from CI or give CI write credentials for built-in releases.

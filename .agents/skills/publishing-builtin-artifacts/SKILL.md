---
name: publishing-builtin-artifacts
description: Builds, validates, publishes, or updates immutable Golem built-in tool and plugin release assets. Use for golem-builtins releases, artifact versions, checksums, provenance, licenses, or builtin-artifacts.json.
---

# Publishing Built-in Artifacts

Built-in component source and build logic stay in the `golem` repository. `golemcloud/golem-builtins` contains only its README and immutable GitHub Release assets. Never copy source there, commit generated WASMs to `golem`, or embed them in a service binary.

Publishing is a manual local operation. Never add a publishing workflow, invoke the publisher from CI, or give CI credentials for `golemcloud/golem-builtins`. CI may validate pins and repository hygiene only.

## Release Contract

- Tag releases as `<component>-v<artifact-version>`. Artifact semver is independent of exported tool versions and must change whenever the bytes change.
- Publish `<component>.wasm`, its `.sha256`, `provenance.json`, applicable licenses, and an SPDX SBOM when one is generated.
- Existing tags and assets are immutable. Fixes require a new version; do not use replacement uploads.
- Stable publication requires release-owner/legal approval and an authenticated local GitHub CLI session with write access to `golemcloud/golem-builtins`.

## Workflow

1. Build and validate the component with its focused `cargo make build-<component>` task.
2. Commit the exact source and build inputs that produced the artifact. The generated WASM stays ignored.
3. From the clean local checkout, dry-run packaging:

   ```shell
   BUILTIN_COMPONENT=<component> BUILTIN_VERSION=<version> BUILTIN_DRY_RUN=1 cargo make publish-builtin-artifact
   ```

4. Review the reported size, SHA-256, source commit, exports, licenses, SBOM, tag, and URL.
5. Publish locally with the same command without `BUILTIN_DRY_RUN` after approval.
6. Update the matching entry in `builtin-artifacts.json`, verify the public URL and checksum, and commit the pin. Registry defaults must always include SHA-256; omit it only for explicit experiments.

The publisher must run from the source commit recorded in provenance. Do not publish from an uncommitted tree and claim the previous commit as its source.

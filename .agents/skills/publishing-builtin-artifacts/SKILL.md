---
name: publishing-builtin-artifacts
description: Builds, validates, publishes, or updates immutable Golem built-in tool and plugin release assets. Use for golem-builtins releases, artifact versions, checksums, provenance, licenses, or builtin-artifacts.lock.json.
---

# Publishing Built-in Artifacts

Built-in component source and build logic stay in the `golem` repository. `golemcloud/golem-builtins` contains only its README and immutable GitHub Release assets. Never copy source there, commit generated WASMs to `golem`, or embed them in a service binary.

Publishing is a manual local operation. Never add a publishing workflow, invoke the publisher from CI, or give CI credentials for `golemcloud/golem-builtins`. CI may validate pins, fetch checksum-pinned public assets for tests, and check repository hygiene.

## Release Contract

- Tag releases as `<component>-v<artifact-version>`. Artifact semver is independent of exported tool versions and must change whenever the bytes change.
- Start a new component artifact at `0.0.1`. Until it defines a separate compatibility policy, increment the patch version for every byte-changing publication.
- Published tool coordinates remain bound to their original component revision. When changed bytes must alter a tool's implementation, bump that exported tool's version too; unchanged coordinates continue using their immutable original revision.
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
6. Update the matching component/version/SHA-256 entry in `builtin-artifacts.lock.json`, verify the public URL and checksum, and commit the lock. The component artifact version is independent from the versions of tools it exports and changes whenever the component bytes change.

The publisher must run from the source commit recorded in provenance. Do not publish from an uncommitted tree and claim the previous commit as its source.

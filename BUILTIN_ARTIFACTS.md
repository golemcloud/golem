# Built-in artifacts

`builtin-artifacts.json` lists the eight components, output paths, build tasks,
and source inputs. Development builds use local files. The cargo-make task
conditions compare source modification times with each output and ignore
Git-ignored build directories. `cargo make ensure-builtin-artifacts` builds missing or changed
outputs. The `cargo make build`, `build-golem`, and `build-golem-cli` entry points
ensure these outputs first. Plain `cargo build` does not prebuild them.

When both JavaScript and TypeScript outputs are stale, preparation runs their
shared build pipeline once. It copies only the stale outputs, so a fresh manual
replacement of the other output remains unchanged.

The warm check starts no component build,
npm install, or Wizer process. It preserves manually replaced
WASMs until their source inputs change. There is no separate build executable,
fingerprint database, or build receipt. The checks use file modification times.
After removing source files, changing build toolchains, or changing SDK override
paths, delete the affected output to rebuild it. CI uses a source-content
and tool-version cache key, builds current sources on a cache miss, and distributes
a checksum-verified archive. Consumers need no component build toolchains.

Registry development defaults use the inventory's local WASM files. Configure
`builtin_artifacts.source_overrides` to select a different source per artifact ID.
Local defaults are absolute `file://` URLs rooted at the source checkout, with no
checksum pin. An override can select another file or a checksum-pinned HTTPS URL:

```toml
[builtin_artifacts.source_overrides.bash]
url = "file:///absolute/path/to/bash.wasm"
# sha256 = "<SHA-256 of the selected file>"
```

To keep replacements at the standard output paths even when source inputs change,
set `GOLEM_BUILTIN_ARTIFACTS_PREBUILT=1` when running cargo-make. This skips all
component builds and requires all eight outputs to exist. A source override at a
different path does not need this switch.

There are no generated development versions. Replacing a WASM changes what the
artifact resolver reads; it does not rebind published tool coordinates in an
existing registry database. Those coordinates remain immutable.

## Local installation

`cargo make install-debug`, `install-dev-release`, and `install-release` prepare
the tools and install `golem` with compressed WASMs through the opt-in
`embed-builtin-artifacts` feature. The installed binary works outside the checkout
and does not need the original WASM files. Reinstall it to update its embedded
defaults. `builtin_artifacts.source_overrides` still takes precedence, so a local
file override can change without rebuilding the binary. The standalone
`golem-cli` does not provision built-ins and needs no embedded artifacts.

Published release binaries do not enable this feature. They use the generated
release manifest and download-and-cache resolver. Setting `GOLEM_BUILTIN_RELEASE`
and enabling `embed-builtin-artifacts` together fails compilation. The install
tasks omit the feature when `GOLEM_BUILTIN_RELEASE` is set.

## Release registry coverage

Run `cargo make builtin-artifacts-release-tests` to compile the registry with the
checked-in release manifest fixture and run the artifact tests. This tests the
embedded HTTPS sources and checksum-pinned cache instead of development file
defaults. Existing download tests cover fetch, checksum rejection, and cache
recovery. CI runs this check after packaging the normal development test binaries.
The task removes its temporary manifest even if the test command fails. It refuses
to overwrite an existing release manifest. It also checks that release mode rejects
WASM embedding. The built-in artifact CI job separately tests embedded local-install
defaults and source overrides with the current WASMs. Release builds validate that
the manifest has exactly the inventory IDs and the correct component mapping.

## Tagged release preparation

The normal path is `prepare-golem-release.yaml`, dispatched on the exact public
Golem tag with `gh workflow run prepare-golem-release.yaml --ref v1.6.0
--repo golemcloud/golem` (one command). There is no separate tag input. It checks
clean source and the merged preparation PR. It runs the full reusable CI preflight
with published six-SDK and shared-crate checks before publication. The protected
`golem-release` job creates or resumes a draft, builds and publishes all eight
inventory entries, and uploads the verified manifest. It never publishes the
Golem draft. The existing native matrix attaches signed and notarized binaries
before human review. A human publishes the draft to start Docker publication,
which checks successful preparation of that exact tag without repeating full CI.

Configure the `golem-release` GitHub Environment for release tags only, preferably
with required reviewers. Its variable `RELEASE_APP_ID` and secret
`RELEASE_APP_PRIVATE_KEY` identify a GitHub App installed on both
`golemcloud/golem` and `golemcloud/golem-builtins` with contents read/write.
Only the protected publication job mints a token scoped to these repositories.
Normal CI has no built-in write token. The publisher's CI guard permits only
`prepare-golem-release.yaml` with `workflow_dispatch` on the matching Golem tag
in `golemcloud/golem`.

See the [release runbook](.agents/skills/releasing-golem/SKILL.md) for the shared
crate and SDK ordering. The commands below remain available for explicitly
authorized local recovery and packaging inspection.

### Local recovery

Use Python 3.14, the component build prerequisites, and an authenticated
`gh` session with write access to `golemcloud/golem-builtins` and
`golemcloud/golem`. Explicit invocation without a safety flag approves publishing;
obtain authorization before running it. Do not bypass the CI guard.
Ensure `python3` selects Python 3.14. The Python source packager uses
`tarfile.extractall(filter="data")` and `hashlib.file_digest`. The runner's
system Python 3.10 rejects a valid symlink in the source archive and lacks
`hashlib.file_digest`. The protected publisher installs Python 3.14 explicitly.

1. Finish release preparation through a PR merged into `main`. When an
   implementation changes, bump its exported tool version in source, registry
   provisioning, and defaults. Exported
   tool versions are distinct from component artifact versions; the latter use
   the Golem tag version without its leading `v` for every component.
2. Commit all source and build inputs. Push the release tag and create the
   corresponding **draft** Golem GitHub release before starting release compilation.
   Check out that exact tag in a clean checkout with full history. `origin` must
   point to `golemcloud/golem`. The task uses the GitHub API to prove that the tag
   identifies the exact head of a PR merged into `golemcloud/golem`'s `main` and
   that its merge commit remains reachable from public `main`. Authenticate `gh`
   with pull requests read access. Tag the preparation-branch head, not its squash
   commit, and confirm the public tag identifies the same commit.
3. Validate or package without external writes:

   ```shell
   BUILTIN_VERSION=1.6.0 cargo make validate-builtin-release
   BUILTIN_VERSION=1.6.0 BUILTIN_DRY_RUN=1 cargo make publish-builtin-artifacts
   ```

   Validation fetches Git refs but does not build. Dry run ensures all local
   outputs once, then runs the existing single-component publisher in dry-run
   mode for each inventory entry. Neither mode publishes, uploads, or creates
   `builtin-artifacts.release.json`.
4. Publish all components manually:

   ```shell
   BUILTIN_VERSION=1.6.0 cargo make publish-builtin-artifacts
   ```

The task deletes the generated outputs and rebuilds them so development WASM
replacements cannot enter a release. It invokes `cargo make publish-builtin-artifact`
with `BUILTIN_COMPONENT`, `BUILTIN_VERSION`, and `BUILTIN_SKIP_BUILD=1`.
It packages all entries with `BUILTIN_DRY_RUN=1` before uploading any built-in.
The upload pass uses those same packages without rebuilding or repackaging.
The existing publisher retains validation, provenance, licenses, source archives,
and applicable SPDX SBOM packaging. Component releases use immutable
`<component>-v<version>` tags. Never replace assets or move tags.

The task verifies every component's full public asset set against the local package
before it writes the schema-version-1 `builtin-artifacts.release.json`. It uploads
that manifest to the Golem release with `gh release upload`, without `--clobber`.
Release CI must download this manifest before compilation and use its pinned
public assets. Publish the draft Golem release only after preparation succeeds,
including native binaries; the publication event starts Docker publication.
Main CI builds current sources.
There is no checked-in main lock. For a local release binary build, download the
manifest beside `builtin-artifacts.json` and set `GOLEM_BUILTIN_RELEASE=1`.
This explicit switch prevents a leftover release manifest from pinning development
builds. Source archives must include the manifest for reproducible release builds.

Publication is not transactional. On partial failure, fix the cause and rerun
from the same source. The publisher verifies all existing assets before uploading
missing assets, including licenses, provenance, source archives, and applicable
SBOMs. It then downloads and verifies the complete asset set. Matching assets are
reused, never overwritten. An existing manifest must also be identical; a rerun
verifies and reuses it rather than replacing it. License and Python source archives
are deterministic so packaging retries do not change their bytes. A mismatch stops
the release.
If only manifest upload failed, retain
the verified local manifest and retry `gh release upload <tag>
builtin-artifacts.release.json --repo golemcloud/golem` without `--clobber`.
Inspect existing release assets before retrying an upload with an uncertain
outcome. Do not commit the generated manifest. A new source or changed bytes
requires a new release tag.

#### Recovery across build hosts

The same source and tool versions do not guarantee identical WASM bytes across
build hosts. Do not replace an existing package when a local rebuild differs.
If publication stopped after a complete package passed the publisher's public
asset verification, explicitly authorized local recovery can retain that package
and publish only the missing components:

1. Retain the original publisher log and the verified package hashes. Download
   every existing asset and compare it with that evidence. Check the exact tagged
   source commit, exports, licenses, applicable SBOM, and source archive. A release
   name or a provenance claim alone does not prove that publication completed.
   If the original verification evidence is unavailable or any asset differs,
   stop and use a new release tag.
2. Validate the exact clean tag with `validate-builtin-release`. Build the missing
   components from that tag. Use `publish-builtin-artifact` with
   `BUILTIN_COMPONENT`, `BUILTIN_VERSION`, and `BUILTIN_DRY_RUN=1` to inspect each
   package. With explicit publication authorization, run the same task without
   the dry-run flag for each missing component. `BUILTIN_SKIP_BUILD=1` is valid
   only for outputs built from this clean tag, not development replacements.
3. Download and verify the full public asset set for every inventory entry.
   Preserve the already verified packages without uploads. Check each public
   WASM and checksum URL. Construct the standard schema-version-1 manifest from
   these verified public hashes, using exactly the inventory IDs and component
   names and the tag version without its leading `v`.
4. Upload the manifest without replacement, then download it and compare its
   bytes. If a manifest already exists, verify that it is identical instead of
   uploading it again. Keep the verification evidence for human draft review.

This is manual recovery, not a mode of `publish-builtin-artifacts`. The all-entry
task rebuilds every component and requires each public package to match its local
package. Do not bypass that check or weaken the CI guard to resume publication.

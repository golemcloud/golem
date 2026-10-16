# Built-in artifacts

`builtin-artifacts.json` lists the eight components, output paths, build tasks,
and source inputs. Development builds use local files. The cargo-make task
conditions compare source modification times with each output and ignore
Git-ignored build directories. `cargo make ensure-builtin-artifacts` builds missing or changed
outputs. The `cargo make build`, `build-golem`, and `build-golem-cli` entry points
ensure these outputs first. Plain `cargo build` does not prebuild them.

The warm check starts no component build,
Docker container, npm install, or Wizer process. It preserves manually replaced
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

There are no generated development versions and no required redeployment or
database reset for this build workflow.

## Release registry coverage

Run `cargo make builtin-artifacts-release-tests` to compile the registry with the
checked-in release manifest fixture and run the artifact tests. This tests the
embedded HTTPS sources and checksum-pinned cache instead of development file
defaults. Existing download tests cover fetch, checksum rejection, and cache
recovery. CI runs this check after packaging the normal development test binaries.
The task removes its temporary manifest even if the test command fails. It refuses
to overwrite an existing release manifest.

## Manual tagged release preparation

Use the component build prerequisites and an authenticated
`gh` session with write access to `golemcloud/golem-builtins` and
`golemcloud/golem`. Explicit invocation without a safety flag approves publishing.
Never run the publisher in CI.

1. Finish release preparation on main. When an implementation changes, bump its
   exported tool version in source, registry provisioning, and defaults. Exported
   tool versions are distinct from component artifact versions; the latter use
   the Golem tag version without its leading `v` for every component.
2. Commit all source and build inputs. Push the release tag and create the
   corresponding **draft** Golem GitHub release before starting release compilation.
   Check out that exact tag in a clean checkout with full history. `origin` must
   point to `golemcloud/golem`. The task refreshes remote main and proves that the
   tag commit is its ancestor. Confirm the public tag identifies the same commit.
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
The existing publisher retains validation, provenance, licenses, source archives,
and applicable SPDX SBOM packaging. Component releases use immutable
`<component>-v<version>` tags. Never replace assets or move tags.

The task verifies public WASM bytes against local hashes
before it writes the schema-version-1 `builtin-artifacts.release.json`. It uploads
that manifest to the Golem release with `gh release upload`, without `--clobber`.
Release CI must download this manifest before compilation and use its pinned
public assets. Publish the draft Golem release only after this upload succeeds;
the release's publication event starts release CI. Main CI builds current sources.
There is no checked-in main lock. For a local release binary build, download the
manifest beside `builtin-artifacts.json` and set `GOLEM_BUILTIN_RELEASE=1`.
This explicit switch prevents a leftover release manifest from pinning development
builds. Source archives must include the manifest for reproducible release builds.

Publication is not transactional. On partial failure, fix the cause and rerun
from the same source; matching public assets are verified and reused, never
overwritten. A mismatch stops the release. If only manifest upload failed, retain
the verified local manifest and retry `gh release upload <tag>
builtin-artifacts.release.json --repo golemcloud/golem` without `--clobber`.
Inspect existing release assets before retrying an upload with an uncertain
outcome. Do not commit the generated manifest. A new source or changed bytes
requires a new release tag.

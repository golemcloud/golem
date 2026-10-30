# Golem development

## Before your first pull request

Golem uses [vouch](https://github.com/mitchellh/vouch): you need to be listed in `.github/VOUCHED.td` to open pull requests, otherwise they are closed automatically.

If this is your first contribution, introduce yourself and ask for permission on [Discord](https://discord.gg/UjXeH8uG4x).

Once a maintainer vouches for you, you can open pull requests normally.

## Prerequisites

To work on **Golem** you need to install (via package installers or manually) the following tools:

- [rustup](https://rustup.rs/)
- For integration tests
  - [redis](https://redis.io/downloads/)
  - [docker](https://www.docker.com)
- To be able to run all services with `cargo-make run` with a merged log view:
  - [lnav](https://lnav.org)
  - [nginx](https://nginx.org)

### OSX
```sh
brew install rustup redis docker lnav nginx
```

## Rust Installation
```sh
# latest **stable** rust compiler
rustup update stable
rustup default stable
rustup target add wasm32-wasip2
cargo install --force cargo-make
```

Everything else is managed by [cargo-make](https://github.com/sagiegurari/cargo-make).

## Development workflow

### Building
To compile everything use

```shell
cargo make build
```
It is recommended to do a full build before starting working on Golem and opening it with an IDE. During development it is usually enough to recompile only the crate you are working on, for example:

```shell
cargo build -p golem-worker-service-base
```

For local server and CLI binaries, use `cargo make build-golem` and
`cargo make build-golem-cli`. These tasks also prepare missing or changed built-in
WASMs. A warm build preserves manually replaced WASMs. Plain `cargo build` does
not prepare built-ins. See [Built-in artifacts](BUILTIN_ARTIFACTS.md) for source
overrides, CI caching, and the tagged release workflow.

#### If cargo runs out of memory
Depending on the number of CPU cores and available memory, building everything can use a lot of memory. If cargo runs out of memory or just uses too much, you can limit the number of parallel jobs by providing a cargo `config.toml` file, for example:

```toml
[build]
jobs = 4
```

in `~/.cargo/config.toml`. For other options and possibilities check [the cargo documentation](https://doc.rust-lang.org/cargo/reference/config.html).

### Running tests

Tests are using the [test-r library](https://test-r.vigoo.dev).

During development you can run the involved tests in the usual ways: from the IDE, or using `cargo test` command selecting a specific crate and test module, for example:

```shell
cargo test -p golem-worker-executor api::promise -- --report-time
```

#### Running all unit tests
To run all unit tests use

```shell
cargo make unit-tests
```

#### Running all worker executor tests
The **worker executor tests** are testing the Golem Worker Executor standalone without any of the other services. These tests require `redis`. To run all of them use

```shell
cargo make worker-executor-tests
```

As there are many of these tests, they are organized into **groups** that are executed in parallel on CI. You can run only a specific group with cargo make, for example:

```shell
cargo make worker-executor-tests-group1
```

#### Running all integration tests
The **integration tests** are starting up several Golem services and testing them together. These tests also require `docker` and `redis` to be available.

To run all integration tests use

```shell
cargo make integration-tests
```

#### Running all the CLI tests
The **CLI tests** are similar to the integration tests but interact with the running services only through the Golem CLI application.

To run all CLI tests use

```shell
cargo make cli-integration-tests
```

For faster local runs with dev-release binaries, use:

```shell
cargo make cli-integration-tests-dev-release
```

#### Running sharding tests
For sharding-related tests with file logging:

```shell
cargo make sharding-tests-debug
```

#### Using a debugger
When using a debugger with the tests, make sure to pass the `--nocapture` option to the test runner, otherwise the debugger will not be usable (when capturing is on, the test framework spawns child processes to run the actual tests).

### Updating the REST API
Golem **generates OpenAPI specs** from the Rust code (using the [poem-openapi crate](https://crates.io/crates/poem-openapi), and the generated OpenAPI yaml file is also stored in the repository and a Rust Client crate is generated from it, used by the CLI app and also published into crates.io.

When changing anything that affects the user facing REST API, this YAML needs to be explicitly regenerated. If this does not happen, the CI process will fail and ask for doing it.

To regenerate the OpenAPI spec use

```shell
cargo make generate-openapi
```

### Updating the config files
Service config files are also generated from code similarly to OpenAPI specs. When changing any of the service configuration data types, they have to be regeneraetd otherwise the CI process fails and asks for doing it.

To regenerate these files, use

```shell
cargo make generate-configs
```

### Preparing the pull request
Golem CI checks the pull requests with `rustfmt` and `cargo clippy`. To make sure these checks pass, before opening a pull request run

```shell
cargo make fix
```

and fix any possible errors and warnings reported by it.

## Release process

The six SDKs release on tag pushes (Go uses module tags). Shared crates release
when you publish their GitHub release. Golem preparation runs manually on its
exact tag, builds the signed binaries, and attaches them to the draft. Docker
publication starts only when a human publishes the prepared GitHub draft release.

| Tag pattern                | What gets released                                                    | Workflow                                |
|----------------------------|-----------------------------------------------------------------------|-----------------------------------------|
| `golem-shared-v<x.y.z>`   | `golem-schema-derive`, `golem-schema`, `golem-tool-metadata` on crates.io | `.github/workflows/publish-golem-shared.yaml` |
| `v<major>.<minor>.<patch>` | Signed Golem and CLI binaries in the draft, then Docker images after publication | `prepare-golem-release.yaml`, then `publish-golem-release.yaml` |
| `golem-rust-v<x.y.z>`      | The Rust SDK (`golem-rust`, `golem-rust-macro`) on crates.io          | `.github/workflows/publish-golem-rust.yaml`    |
| `golem-ts-v<x.y.z>`        | The TypeScript SDK packages on npmjs                                  | `.github/workflows/publish-golem-ts.yaml`      |
| `golem-effect-v<x.y.z>`    | The Effect SDK (`@golemcloud/effect-golem`) on npmjs                  | `.github/workflows/publish-golem-effect.yaml`  |
| `golem-scala-v<x.y.z>`     | The Scala SDK on Maven Central                                        | `.github/workflows/publish-golem-scala.yaml`   |
| `golem-moonbit-v<x.y.z>`   | The MoonBit SDK on mooncakes.io                                       | `.github/workflows/publish-golem-moonbit.yaml` |
| `sdks/go/{core,golem,bridge}/v<x.y.z>` | The three Go SDK modules through the Go proxy              | No publish workflow; push all three module tags |

Most package manifests use placeholder versions. Release workflows replace them
from the tag. There is no shared-version pin file. Consumers select the nearest
`golem-shared-v*` tag on first-parent history. Selection fails if no such tag exists
or if the selected commit has multiple shared tags. The tag supplies the version;
shared source snapshot and registry provenance checks still apply.
`cli/golem-cli/src/versions.rs` records the six SDK versions that the CLI uses.
Go module requirements must contain their release versions before you tag them.

### Complete release order

At release start, select `main` or the matching `<major>.<minor>.x` maintenance
branch. Record its current public commit and create a preparation branch from it.
Keep release fixes and version pins on that preparation branch. Do not merge or
rebase later development into the release source. Release authorization does not
authorize direct pushes or merges to the selected target branch.

1. **Freeze and publish the shared crates.** Choose an unused crates.io version
   and commit the shared crate sources.
   The version can equal the planned Golem version. The publication workflow
   gates uploads on the release-tooling checks and shared-library tests. Full
   platform CI is not a prerequisite for publishing the shared crates.
   Push `golem-shared-v<version>` at that commit and publish its GitHub release.
   The release-published event starts the workflow, which tests the libraries
   and publishes `golem-schema-derive`, then `golem-schema`, then
   `golem-tool-metadata`. Mark an RC as a prerelease, not the latest release.
   Do not create a Golem GitHub release for this step.
2. **Publish the six SDKs.** The Rust SDK selects the shared tag from first-parent history.
   It requires the shared tag, unchanged sources, and published crates whose
   embedded source commit matches the tag. The other SDKs can publish in parallel.
   Push each workflow-driven SDK tag separately and confirm its workflow starts.
   GitHub does not create tag-push events for pushes with more than three tags.
   Do not combine the five SDK tag pushes with the Go tags or use `git push --tags`.
   For Go, replace the core requirements with the release version. Keep the local
   replacements for in-repo builds. Push all three Go tags at the
   same commit. See the `golem-go-development` skill for the module release steps.
3. **Pin and test the published SDKs.** Update all six pins in
   `cli/golem-cli/src/versions.rs`. You can run CI manually on the preparation
   branch for early feedback. The tagged preparation in the next step is required.
   Its `release-prerequisites` job verifies the shared crates and runs the
   six streaming template walkthroughs with registry SDKs. Each walkthrough
   creates a project, builds it, deploys it, and checks invocation results.
   The job clears every SDK override, including the Go workspace fallback, and
   does not restore locally published Scala SDKs. Manual CI does not publish
   crates, binaries, or Docker images.
4. **Prepare Golem.** Push the immutable Golem tag after all pins and exported
   built-in tool versions are committed. First use a reviewed PR and obtain
   explicit approval to squash merge the preparation branch into the selected
   target branch.
   Record the PR's exact head before merging. Tag that preparation-branch commit,
   not the squash commit on the target branch, to preserve the frozen release
   source and its first-parent shared snapshot. Run
   the preparation workflow on that exact public tag (there is no separate
   tag input):

   ```shell
   gh workflow run prepare-golem-release.yaml --repo golemcloud/golem --ref v1.6.0
   ```

   `.github/workflows/prepare-golem-release.yaml` validates the exact public tag,
   clean source, and the merged preparation PR. The PR must have the tagged commit
   as its exact head and target `golemcloud/golem`'s `main` or the maintenance
   branch matching the tag (for example, `1.6.x` for `v1.6.2`). Its merge commit
   must remain reachable from that public target branch. It runs the full reusable
   CI preflight with published SDK and shared-crate checks. Its protected
   `golem-release` job creates or resumes the draft, builds and publishes all
   eight built-in inventory entries, and uploads the verified manifest. All
   built-in packages must finish before the first built-in upload. The existing native matrix then builds,
   signs, notarizes, and attaches all Golem and CLI binaries to the draft.
   Generated notes start at the highest earlier published stable Golem version,
   excluding SDK releases, shared crate releases, drafts, and prereleases.
   The workflow passes that tag explicitly because release tags on squash-merged
   PR heads do not form an ancestor chain. It removes the previous preparation
   PR from the generated list and includes the current preparation PR once.
   It resolves each PR from its tagged head on `main` or the matching maintenance
   branch. Other generated entries and the tag comparison stay unchanged.
   Preparation stops if a tagged head does not identify one merged preparation PR.
   Existing drafts keep their notes. Review the notes before publication.
   New drafts explicitly disable the Latest designation. Preparation never
   publishes the Golem draft.
5. **Review and publish Golem.** Wait for successful preparation. Review the draft,
   binaries, and manifest, then explicitly publish the draft with
   `gh release edit <tag> --repo golemcloud/golem --draft=false --latest=false`.
   In the GitHub UI, uncheck **Set as latest release** before publishing.
   Only the publication workflow must promote Latest; a manual promotion can
   race its channel selection. Do not rely on the draft's initial Latest flag.
   Publication starts `publish-golem-release.yaml`, which requires successful preparation of that
   exact tag before running the existing Docker jobs. It does not repeat full
   CI, the prerequisite checks, native builds, or shared crate publication.
   After validation, it sets the GitHub Latest designation only for the highest
   published stable Golem version. Docker uses the same version policy for `:latest`.

Run `cargo make check-release-start` before starting the release. It checks clean
source, GitHub authentication, and the protected environment's App configuration
names without publication. It cannot validate the private key or registry access.

After shared crate publication, do not change the three crate directories, the
root `Cargo.toml`, `Cargo.lock`, `.cargo`, or `LICENSE` before releasing the SDK
and Golem. These files define the shared source and dependency snapshot. CLI pin
changes do not alter that snapshot. If the snapshot must change, use a new shared
version and tag, then release the Rust SDK against that version.

Never replace published crate versions. Keep release tags immutable except for
the failed pipeline-test case below. If a shared release fails after publishing
some crates, rerun its workflow at the same tag. It verifies
existing packages against their registry checksums and embedded source commits,
then publishes only missing packages. A conflicting package stops the release.
Version stamping changes manifests during packaging, so Cargo marks packages as
dirty; the publisher requires a clean tagged checkout before that stamping step.

During an explicitly identified pipeline test, a human can approve deleting and
recreating a failed shared tag and GitHub release at a fixed commit. First verify
that all three crate versions are absent from crates.io, the release has no assets,
and no SDK or Golem release consumes the failed tag. General release authorization
does not cover this exception. Once any crate is published, preserve the tag.

Configure the GitHub Environment `golem-release` to permit only release tags;
required reviewers are recommended. Set its variable `RELEASE_APP_ID` and secret
`RELEASE_APP_PRIVATE_KEY` for a GitHub App installed on `golemcloud/golem` and
`golemcloud/golem-builtins` with contents read/write permission. Only the protected
publication job mints the scoped token. Normal CI has no built-in write token.
The CI publication guard accepts only `workflow_dispatch` from
`prepare-golem-release.yaml` on the matching Golem tag in `golemcloud/golem`.
Local explicit publication and dry runs remain available for recovery with the
same immutability safeguards. Partial reruns verify and reuse identical assets,
including an existing manifest; they never replace them.

See the [release runbook](.agents/skills/releasing-golem/SKILL.md) for commands,
authorization gates, and recovery, and [Built-in artifacts](BUILTIN_ARTIFACTS.md)
for packaging details.

npm packages use `next` for prereleases (for example, `2.0.0-rc1`). A stable
version uses `latest` unless the package already has a higher stable version in
the registry. In that case, it uses `release-<major>.<minor>` (for example,
`release-1.6`). Each package selects its channel independently. SDK versions need
not match the Golem version: the Golem 1.6 line can use an SDK 2.0 patch release.
An npm release fails if a newer stable version already exists in the same
`major.minor` line, or if a newer prerelease already exists for `next`.
Prerelease comparison uses semantic version ordering, including numeric identifiers.
The TypeScript workflow validates all package channels before it publishes any
package. This prevents lookup or channel validation errors from causing partial
publication; registry publication itself is not atomic.
Registry or GitHub lookup failures stop publication rather than assume that
`latest` is safe.
Publication workflows serialize releases across lines so channel selection and
uploads do not race. GitHub Actions retains only one pending run per concurrency
group: wait for a queued publication to start before triggering another one.

### Maintenance branches

For each shipped `major.minor` line that still needs bug-fix releases, we keep a
long-lived `<major>.<minor>.x` branch (for example, `1.6.x`). Create it from an
agreed public `main` commit before breaking development for the next line starts.
The repository rules already cover maintenance branches, including restricted
branch creation. An authorized bypass actor must create the branch.
Land release-tooling changes on both active lines before tagging new releases.
The preparation workflow must exist on the default branch and at the selected tag.
Maintenance Golem preparation requires the exact head of a PR merged into the
matching public maintenance branch. It does not require merging that source into `main`.

```shell
# After the reviewed preparation PR merges into 1.6.x, tag its recorded head.
git tag v1.6.2 <approved-preparation-pr-head>
git push origin v1.6.2
gh workflow run prepare-golem-release.yaml --repo golemcloud/golem --ref v1.6.2
```

The CI workflows accept push events from `main` and any `<major>.<minor>.x`
branch automatically.

**Backports** between branches are done manually with `git cherry-pick`. The
usual flow is: land the fix on `main` first, then cherry-pick the commit(s)
onto the relevant maintenance branch(es) via a separate PR.

**Default release channels** depend on published stable versions, not branch
ancestry. While Golem 2.0 is prerelease-only, new Golem 1.6 patches still update
the GitHub and Docker stable defaults. Once stable Golem 2.0 ships, later 1.6
patches do not replace it. npm applies this policy to each SDK's own versions.
Versioned releases and images remain available.

## Running Golem locally

There are two ways now to run Golem locally:

### Using cargo make run

By running `cargo make run` all services are going to be built and started as individual native processes.
Ensure that `lnav`, `nginx` and `redis` commands are available in your PATH.

### Using cargo make run-with-login-enabled

By running `cargo make run-with-login-enabled`, all services are started as individual native processes
and the login system is enabled. In addition to the [requirements for regular run](#using-cargo-make-run), you'll need to set up GitHub OAuth.

1. Go to https://github.com
2. Navigate to `Settings > Developer settings > OAuth Apps`
3. Create a new OAuth App. Use the following settings:

    Authorization callback URL: `http://localhost`
4. Generate a new client secret.
5. Export the following environment variables:

    GITHUB_CLIENT_ID: Client ID of the OAuth application created in step (3)

    GITHUB_CLIENT_SECRET: Client secret created in step (4)

After it is started you can configure the cli to use the local instance using `golem profile new --component-url http://localhost:9881/  cloud cloud-local`.

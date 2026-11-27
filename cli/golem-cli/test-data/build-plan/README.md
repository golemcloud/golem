# Host build planning fixtures

`host-action.json` is an early action fixture for the Standard tools resolver.
It describes a host observation, not Tool admission or a supplier declaration.
The Node action can execute arbitrary application code, access host files and
fetch dependencies. Resolving `node` does not prove that its script will succeed.
GC-06 must replace host resolution with the shared resolver and validated
declarations before Tool execution. Do not infer a Tool alias, installed version
or permission from the executable name in this fixture.

## Standalone inspection and execution

Standalone builds retain the existing manifest migration flow, including
`build --step check`: show the conversion diff, ask for confirmation, apply the
conversion, then load the project and run checks. `--yes` confirms the migration.
Without confirmation, an old manifest is left unchanged and checks do not run.
The CLI checks the original manifest bytes again after confirmation, before its
first conversion write. An authorized migration remains persisted if a later
check or build fails.

After loading, `build --step check` inspects loading and dependency repairs
without applying them. It resolves declared host requirements, but does not
generate common files, install dependencies or create the Claude skills symlink,
even with `--yes`. Repeated checks preserve stale task markers too. If repairs
remain, check reports that it did not construct an executable plan. Otherwise it
describes the same immutable inventory that execution consumes. Check does not
run recipes or extract future component metadata.

A build checks its declared requirements before it applies the remaining load
plan or enters grant preparation. This inspection follows any confirmed startup
migration; a missing program does not roll back that migration. This is not a
transaction and it does not roll back later build failures.

After confirmation, a build applies project repairs and reloads persisted files
before constructing its executable plan. No unapplied conversion or dependency
patch is an input to execution. An authorized repair remains persisted if a
later recipe fails.

The inventory contains the selected recipes in their original order, conditional
shared dependency preparations, metadata extraction, bridge collections and known
candidate scopes. Typed inputs refer to future metadata and guest bridges, not
to additional work discovered during execution. Progress and results are separate
from the plan. The serial runner chooses ready components in stable name order.
It retains completed commands, bridge results and preparation results.

Remote tool releases, remote middleware releases and the environment tool
resolver are fixed preparation entries. The inventory shows release references,
component dependency inputs, bridge matchers, owner metadata selectors and the
environment resolver's bounded name/wildcard and MCP import scope. Remote reads
and MCP resolution are distinct from conditional persistent grant upserts.
Import identities include their index and prefix, not authentication values.

Before a stage runs, its selected preparation inputs must have results. New
component metadata can select another inventoried release or activate the fixed
environment resolver; it cannot add a release reference or resolver operation.
Selected identities and result bounds are checked before grant writes or the
next stage. A missing selected environment tool fails before its consumers run.
Grant reconciliation still needs explicit authorization and keeps its changes
if a later build fails. A metadata lookup error is a failure, not authority to
plan another operation. Checks print the inventory without resolving remote
metadata or changing grants.

Middleware lookup may wait for a pending candidate, use one known matching
provider, or reject a known ambiguity or incompatible version. Final validation
checks the complete scope for late duplicates and presentation differences. An
unseeded dependency chain is blocked, not a completed search with no match. A
bridge with a middleware chain waits for every required middleware input.

A missing program or unclassifiable command fails requirement inspection before
the remaining project repairs or recipe execution. A bridge-only build checks
its supplied artifact without requiring an unused compiler pipeline. Conditional middleware
candidates reserve requirements without requiring unrelated artifacts. When a
metadata result activates a candidate, execution checks its artifact before the
next stage.

Installed packages use the same verified declaration projection during check and
build. Inspection reads the pins, lock and package bytes but does not allocate an
unpack root, replace cache entries or write a generated manifest. Package origins
are declaration sources, not editable user manifests. Relative asset defaults
remain anchored to their package declaration source.

The app handler prepares packages automatically before constructing the actual
build plan. Package cache, root and unpack operations are separate from
`BuildPlan` and `BuildExecution`. The preparation inventory describes package
assets before they exist. Its concrete namespace derives from the verified
package identity, not a random temporary path. Preparation revalidates manifest
bytes and locked package identities under the application operation lock.
The command releases the lock but retains the prepared outputs for subsequent invocations.
Task markers bind the inputs and CLI version; every reuse also verifies output
paths, regular file types, sizes and modification times. This check compares
each output with its recorded metadata, not with the age of the input archive.
Missing or changed outputs invalidate readiness. Archive digest and identity
verification remain separate from this cheap extraction freshness check. Edits
that preserve both file size and modification time are not detected.

Task execution takes a closure. It invalidates the previous completion before
the closure starts and atomically publishes the new result afterwards. A killed
or cancelled rerun cannot leave its previous successful completion usable.
Dependency tasks that intentionally rewrite their inputs, such as `go mod tidy`
and npm lockfile creation, explicitly record the final input identity after
success. Ordinary build tasks retain their original input identity.

The bounded preparation sweep admits obsolete candidates during inspection and
never removes the valid current root or adds newly discovered candidates.
Build, install, clean and local artifact consumers serialize their local operation
sections with `golem-temp/operation.lock`. Acquisition has a bounded, cancellable
wait. A terminated holder releases its OS lock; the next command repairs incomplete
outputs using task markers. Clean does not inspect or unpack package archives.
It validates explicit targets before deleting anything and preserves the stable
operation lock so waiters cannot acquire a replaced file.

Read-only build inspection does not create the application lock. Execution acquires
it, reloads and checks requirements again before applying repairs. A confirmed
startup migration acquires protection before writing and loading the converted
manifests. Declining migration writes nothing. REPL preparation and reload hold
local sections, not a session-long lock. Each operation owns a local guard; Context
does not retain one. Startup common-file generation acquires only when its inspected
load plan needs changes, then refreshes and applies the plan under the guard.
It does not authorize manifest migration; MCP calls reject a newly outdated manifest.
Reload preserves the original invocation directory and component selection.
Deploy takes ownership and refreshes requirements before planning or applying grants.
It holds that section through build or package preparation and staging's final
local artifact reads, then releases it before activation and post-deploy actions.
Manifest trace protects metadata extraction; REPL setup protects directory and
history creation separately from build, deployment and the running session.
Remote streams and idle tool servers do not hold local protection. Standalone
custom commands protect dependency preparation; declared targets or directory writes keep the section
through command completion. Custom build steps retain the enclosing build section.
Same-application mutating CLI recursion inside a held section is unsupported and
fails with a contention diagnostic. There is no lock inheritance or bypass flag.

Package archives have separate per-digest cache locks because different applications
can share the cache. Verified warm reads do not acquire a population lock. On a
miss, population acquires the digest lock, rechecks, fetches and verifies before
atomic publication. It holds at most one digest lock and releases it before
extracting assets. Read-only inspection never populates the cache or creates a
cache lock. These locks do not coordinate arbitrary editors, external programs
or different applications that deliberately share output paths.

Pack, publish dry-run and deployment without a build call the same preparation
operation without running component recipes. Installed metadata does not execute
the component to rediscover declarations already verified in the package. Install
uses the shared output policy, including structured success and quiet diagnostics;
project-relative persisted package locators use Unix separators.

Managed Go inspection accepts either the installed toolchain or the pinned
download on a supported platform. Check does not download or install it. A cold
offline build refuses before project writes; an online build retains on-demand
installation, and a warm offline build can use that installation. An explicit
`GOLEM_GO_TOOLCHAIN` override must name an existing toolchain.

`middleware-bootstrap/` supplies a small real Rust component for cold CLI tests.
Its variants export tools, middleware names and aliases. The tests start without
metadata or generated bridges and exercise both asymmetric bootstrap orders,
late duplicates, blocked chains, completed missing searches and explicit scopes.
Separate host recipes exercise shared preparations, command order, failed
producers and deterministic plans despite reversed manifest insertion order.

Native metadata extraction and preinitialization are identified as actions that
execute user components. This host fixture does not qualify fuel, memory or
time limits for those native backends. Host action classification is not Tool
admission or a complete compiler/supplier inventory. Tool execution and its
bounded shared-engine capability remain separate integration work.

## Concrete 1.6 conversion input

`legacy/` and `current/` provide a manifest, build script, package manifest and
lockfile pair for the bridge. Both scripts copy a supplied valid `seed.wasm`;
neither installs a compiler or resolves dependencies. The application version in
the package manifest is a fixture version, not a published compiler version.

Apply these rules together, before running the converted build:

1. Keep `componentWasm` as the artifact used for extraction and deployment.
   Remove `outputWasm` and advance `manifestVersion` to `2.0.0`.
2. Remove the obsolete staged path from command targets. Remove the script's
   copy to that path. A build script is opaque to manifest conversion: review
   its contents, do not replace arbitrary matching text.
3. Preserve the script entry point, its source inputs and the actual component
   output. Resolve the script's program as a requirement before execution.
4. When changing the application package version, update both root versions in
   `package-lock.json` to match `package.json`. Preserve dependency versions,
   source identities and integrity values in a real lockfile; do not refresh
   them during build preflight. This minimal pair has no external packages.
5. Reject remaining manifest references to the removed staged path, including
   references in included manifests and presets. The existing manifest upgrade
   planner checks those references. It deliberately cannot inspect arbitrary
   program behavior or rewrite lockfiles and scripts safely.

These are bridge inputs and conversion rules, not a new automatic migration or
compatibility path. No compiler package, pipeline default or Tool mapping is
selected by this pair.

# Host build planning fixtures

`host-action.json` is an early action fixture for the Standard tools resolver.
It describes a host observation, not Tool admission or a supplier declaration.
The Node action can execute arbitrary application code, access host files and
fetch dependencies. Resolving `node` does not prove that its script will succeed.
GC-06 must replace host resolution with the shared resolver and validated
declarations before Tool execution. Do not infer a Tool alias, installed version
or permission from the executable name in this fixture.

## Standalone inspection and execution

`build --step check` inspects loading and dependency repairs without applying
them. It resolves declared host requirements, but does not convert manifests,
generate common files, install dependencies or create the Claude skills symlink,
even with `--yes`. Repeated checks preserve stale task markers too. If repairs
remain, check reports that it did not construct an executable plan. Otherwise it
describes the same immutable inventory that execution consumes. Check does not
run recipes or extract future component metadata.

A build checks its declared requirements before it applies the load plan or
enters grant preparation. Manifest conversions still show a diff and require
confirmation; `--yes` confirms them. The CLI checks the original manifest bytes
again after confirmation, before its first conversion write. This is not a
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
project repair or recipe execution. A bridge-only build checks its supplied
artifact without requiring an unused compiler pipeline. Conditional middleware
candidates reserve requirements without requiring unrelated artifacts. When a
metadata result activates a candidate, execution checks its artifact before the
next stage.

Installed packages use the same verified declaration projection during check and
build. Inspection reads the pins, lock and package bytes but does not allocate an
unpack root, replace cache entries or write a generated manifest. Package origins
are declaration sources, not editable user manifests. Relative asset defaults
remain anchored to their package declaration source.

The app handler prepares packages automatically before constructing the actual
build plan. Package cache, root, unpack and lease operations are separate from
`BuildPlan` and `BuildExecution`. The preparation inventory describes package
assets before they exist. Its concrete namespace derives from the verified
package identity, not a random temporary path. Preparation revalidates manifest
bytes and locked package identities, then allocates under the registry lock.
Concurrent consumers share verified assets under shared leases. Context exit
releases the lease but retains the prepared outputs for subsequent invocations.
Task markers bind the inputs and CLI version; every reuse also verifies output
presence, asset bytes and the declaration snapshot. Missing or changed outputs
invalidate readiness. Repair refuses an active conflicting root.

The bounded preparation sweep admits obsolete candidates during inspection,
rechecks their leases under the registry lock and never removes the valid current
root or adds newly discovered candidates. Clean does not inspect or unpack package
archives and refuses while a consumer holds a root lease. It retains the stable
registry namespace so waiting preparers cannot acquire a replaced lock.

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

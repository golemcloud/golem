# Host build planning fixtures

`host-action.json` is an early action fixture for the Standard tools resolver.
It describes a host observation, not Tool admission or a supplier declaration.
The Node action can execute arbitrary application code, access host files and
fetch dependencies. Resolving `node` does not prove that its script will succeed.
GC-06 must replace host resolution with the shared resolver and validated
declarations before Tool execution. Do not infer a Tool alias, installed version
or permission from the executable name in this fixture.

## Standalone inspection and execution

`build --step check` loads proposed manifest conversions and common templates in
memory. It resolves the declared host programs and reports dependency repairs.
It does not apply conversions, generate common files, install dependencies or
create the Claude skills symlink, even with `--yes`. A repeated check has the
same no-write contract.

A build checks its declared requirements before it applies the load plan or
enters grant preparation. Manifest conversions still show a diff and require
confirmation; `--yes` confirms them. The CLI checks the original manifest bytes
again after confirmation, before its first conversion write. This is not a
transaction and it does not roll back later build failures.

The plan reuses dependency preparation and the existing component scheduler.
Pending stages are revalidated before the next component executes. A missing
program or unclassifiable command fails preflight rather than starting earlier
commands. A bridge-only build checks its supplied artifact without requiring an
unused compiler pipeline. Managed Go inspection requires an installed toolchain
and does not bootstrap one as a side effect of checking it.

Native metadata extraction and preinitialization are identified as actions that
execute user components. This host fixture does not qualify fuel, memory or
time limits for those native backends, nor does it qualify complete bridge-action
coverage. Tool execution and its bounded shared-engine capability remain separate
integration work.

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

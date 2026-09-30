# Built-in tools

Built-in tools and middlewares are component sources built from this repository, published as
immutable releases in [`golemcloud/golem-builtins`](https://github.com/golemcloud/golem-builtins),
downloaded into the registry-service artifact cache, and provisioned at registry startup.
`BUILTIN_EXPORTS` in `golem-registry-service/src/services/builtin_tool_provisioner.rs` is the
production export inventory; `builtin-artifacts.lock.json` pins each component artifact's
independent version and SHA-256. The registry derives the canonical GitHub release URL from that
lock. Server configuration is only for deployment-specific cache placement and source overrides;
it does not duplicate the production release catalog.

The filesystem component is implemented in Rust under `builtin-tools/filesystem-tools/`. The same
component provides the `read-file`, `write-file`, `edit-file`, `ls`, and `grep` tools and the
universal `path-policy` middleware.

The JavaScript and TypeScript tools are implemented under `builtin-tools/js-ts-tools/`. The
`javascript-tools` component provides `node`, `npm`, and `npx`; the `typescript-tools` component
provides `tsc`. `node` provides JavaScript execution through Golem's QuickJS-based Node-compatible
runtime. The upstream npm and tsc executable graphs are Rollup-bundled and embedded in their
components; they do not provision an npm or TypeScript package tree into the invoking agent.
TypeScript's standard-library declarations and npm's manual pages are embedded as private read-only
data. Both components must use the `optimized` TypeScript preset so Wizer pre-initializes their
provider state.

## Adding a component-implemented built-in export

1. Add a standalone tool component source and build it through its Golem application manifest. Do
   not invoke its language compiler directly.
2. Add its logical artifact ID to `BuiltinExportDescriptor`; keep export metadata in the descriptor
   and add the component name, independent artifact version, and checksum to
   `builtin-artifacts.lock.json`.
3. Set `component_name`, `export_name`, `release_version`, and `kind` to the artifact's exported
   metadata. Provisioning validates these values before writing anything.
4. Add an independent build task and include it in `build-builtin-tools`. Validate the generated
   WASM and run the registry provisioning tests.
5. Commit the exact source and build inputs, then manually publish an immutable component release
   from that clean local checkout with `cargo make publish-builtin-artifact`. Commit the resulting
   version/checksum update to `builtin-artifacts.lock.json` separately. Generated WASMs are never
   committed.

Provisioning is idempotent for identical bytes and an identical exact version. A published system
release is protected and immutable: repeat startup with the same version only when the artifact and
metadata are identical. For any changed artifact or metadata, publish a new version; do not replace
the existing coordinate. New component artifacts start at `0.0.1`. Until an artifact defines a
separate compatibility policy, increment its patch version for every byte-changing publication;
artifact versions do not determine the versions of the tools or middlewares exported by the
component.

Component-implemented built-ins are grantable registry releases, not ambient tools. A consuming
manifest must select each exact tool release under `tools.<name>.release` and each middleware
release under `tools.middleware.<name>.release`. Native tools compiled into the host use separate
registry/executor startup inventories and are ambient, so they have no top-level release
declaration. See `golem-native-tool/README.md` for that path.

For example, this selects the built-in `read-file` release and makes it available to an agent:

```yaml
manifestVersion: 1.6.0

app: filesystem-reader

components:
  example:filesystem-reader:
    templates: rust
    dir: .

tools:
  read-file:
    release:
      account: builtin-tool-owner@golem.cloud
      name: read-file
      version: 0.1.0
  middleware:
    path-policy:
      release:
        account: builtin-tool-owner@golem.cloud
        name: path-policy
        version: 0.1.0

environments:
  local:
    tools:
      middleware:
        - name: path-policy
          version: 0.1.0
          parameters:
            base: /workspace
            allowed_roots:
              - path: project
                operations: [read]
          filesystemAccess: allowed

agents:
  FileReader:
    tools:
      read-file:
        filesystemAccess: allowed
```

This assumes the component exports the `FileReader` agent and that its files are rooted under
`/workspace`. The policy resolves relative invocation paths and relative allowed roots against
`base`. Each root independently grants `read`, `write`, and/or `delete`; `read-file` and `ls` use
`read`, `grep` uses `read`, `write-file` and `edit-file` use `write`, and `delete-file` uses
`delete`. Unknown tool names pass through unchanged. Recognized filesystem tool names are accepted
only at their root command and must have a string `path` argument.

Both filesystem grants are intentional. The agent's `read-file` binding needs
`filesystemAccess: allowed` so the leaf tool can read the owner filesystem. The `path-policy`
middleware occurrence independently needs `filesystemAccess: allowed` so it can inspect existing
path components and reject symbolic links. Without the middleware grant, protected calls fail
closed with `path-policy-denied` before leaf dispatch because the policy cannot inspect the owner
filesystem root.

`ls` and `grep` intentionally discover paths below their explicit root argument. A middleware that
allows such a call authorizes inspection of the selected subtree; include/exclude globs select
results but are not an authorization boundary. The tools never follow symbolic links.

The middleware normalizes `.` and `..` components without allowing traversal above the owner root,
checks containment by path components, rejects backslashes and NUL bytes, and rejects any existing
symbolic-link component, including a dangling final link. A write below a nonexistent suffix is
allowed only when its nearest existing ancestor is link-free and the normalized target remains
inside a write-enabled root. Allowed calls receive the normalized absolute path; denied calls
return `path-policy-denied` before invoking the wrapped tool.

This is argument-level policy for the named filesystem tools, not a filesystem sandbox. It does
not restrict arbitrary filesystem behavior inside a wrapped tool and does not prevent another
component with its own filesystem capability from accessing paths directly.

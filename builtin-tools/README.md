# Built-in tools and middleware

Built-in tools are tool components shipped as bytes in the registry-service binary and provisioned
at registry startup. `BUILTIN_EXPORTS` in
`golem-registry-service/src/services/builtin_tool_provisioner.rs` is the production inventory.

The filesystem component is implemented in Rust under `builtin-tools/filesystem-tools/` and built
into `builtin-tools/filesystem-tools.wasm`. The same component provides the `read-file`,
`write-file`, and `edit-file` tools and the universal `path-policy` middleware.

The reusable Audit policy is implemented under `builtin-tools/audit-middleware/` and built into
`builtin-tools/audit-middleware.wasm`. Its package README defines the sink and idempotency contract
and includes a runnable duplicate-occurrence binding.

The reusable output-redaction middleware lives under `builtin-tools/output-redaction/` and builds
to `builtin-tools/output-redaction.wasm`. It is published and installed through the normal tool
middleware release and binding flow; unlike the universally provisioned filesystem releases, it
is not automatically installed into environments. Its README defines the supported selector and
literal-pattern policy and includes a complete binding example.

## Adding a component-implemented built-in export

1. Add a standalone tool component source and build it through its Golem application manifest. Do
   not invoke its language compiler directly.
2. Add the committed WASM to `BuiltinExportDescriptor`, using `include_bytes!` so startup never
   depends on a filesystem path.
3. Set `component_name`, `export_name`, `release_version`, and `kind` to the artifact's exported
   metadata. Provisioning validates these values before writing anything.
4. Add the component to `build-builtin-tools` in the root `Makefile.toml` and run
   `cargo make build-builtin-tools`. Validate the resulting WASM and run the registry provisioning
   tests.
5. Commit the source, manifest, descriptor, and rebuilt WASM together.

Provisioning is idempotent for identical bytes and an identical exact version. A published system
release is protected and immutable: repeat startup with the same version only when the artifact and
metadata are identical. For any changed artifact or metadata, publish a new version; do not replace
the existing coordinate.

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
      version: 0.3.0
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
`read`, `write-file` and `edit-file` use `write`, and `delete-file` uses `delete`. Unknown tool names
pass through unchanged. Recognized filesystem tool names are accepted only at their root command
and must have a string `path` argument.

Both filesystem grants are intentional. The agent's `read-file` binding needs
`filesystemAccess: allowed` so the leaf tool can read the owner filesystem. The `path-policy`
middleware occurrence independently needs `filesystemAccess: allowed` so it can inspect existing
path components and reject symbolic links. Without the middleware grant, protected calls fail
closed with `path-policy-denied` before leaf dispatch because the policy cannot inspect the owner
filesystem root.

The middleware normalizes `.` and `..` components without allowing traversal above the owner root,
checks containment by path components, rejects backslashes and NUL bytes, and rejects any existing
symbolic-link component, including a dangling final link. A write below a nonexistent suffix is
allowed only when its nearest existing ancestor is link-free and the normalized target remains
inside a write-enabled root. Allowed calls receive the normalized absolute path; denied calls
return `path-policy-denied` before invoking the wrapped tool.

This is argument-level policy for the named filesystem tools, not a filesystem sandbox. It does
not restrict arbitrary filesystem behavior inside a wrapped tool and does not prevent another
component with its own filesystem capability from accessing paths directly.

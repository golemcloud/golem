# Built-in tools

Built-in tools are tool components shipped as bytes in the registry-service binary and provisioned
at registry startup. `BUILTIN_TOOLS` in
`golem-registry-service/src/services/builtin_tool_provisioner.rs` is the production inventory.

The filesystem tools are implemented in Rust under `builtin-tools/filesystem-tools/` and built
into `builtin-tools/filesystem-tools.wasm`. The component provides the `read-file`, `write-file`, and
`edit-file` tools.

## Adding a component-implemented built-in tool

1. Add a standalone tool component source and build it through its Golem application manifest. Do
   not invoke its language compiler directly.
2. Add the committed WASM to `BuiltinToolDescriptor`, using `include_bytes!` so startup never
   depends on a filesystem path.
   If a tool needs immutable runtime files, also commit one ZIP beside the WASM, embed it through
   `files_archive_bytes`, and map only that tool's required entries through a committed JSON
   `files_manifest_bytes`. Components may share one archive while giving different tools
   different file sets.
3. Set `component_name`, `tool_name`, and `release_version` to the artifact's exported metadata.
   Provisioning validates these values before writing anything.
4. Add the component to `build-builtin-tools` in the root `Makefile.toml` and run
   `cargo make build-builtin-tools`. Validate the resulting WASM and run the registry provisioning
   tests.
5. Commit the source, manifest, descriptor, and rebuilt WASM together.

Provisioning is idempotent for identical bytes and an identical exact version. A published system
release is protected and immutable: repeat startup with the same version only when the artifact and
metadata are identical. For any changed artifact or metadata, publish a new version; do not replace
the existing coordinate.

Component-implemented built-ins are grantable registry releases, not ambient tools. A consuming
manifest must select the exact release under `tools.<name>.release` **and** bind that logical name
under `agents.<agent>.tools`. Native tools compiled into the host use separate registry/executor
startup inventories and are ambient, so they have no top-level release declaration. See
`golem-native-tool/README.md` for that path.

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

agents:
  FileReader:
    tools:
      read-file:
        filesystemAccess: allowed
```

This assumes the component exports the `FileReader` agent. The built-in filesystem tools require
filesystem access but provision no files of their own. This example explicitly grants
`filesystemAccess: allowed` on the agent binding; if it is omitted and no grant is inherited,
deployment fails with an error requesting that permission. This fails closed rather than exposing
the agent's filesystem.

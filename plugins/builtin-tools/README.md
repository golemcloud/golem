# Built-in tools

Built-in tools are tool components shipped as bytes in the registry-service binary and provisioned
at registry startup. The Bash tool is built from `plugins/builtin-tools/bash`, committed as
`plugins/builtin-tools/bash.wasm`, and published as the protected system release `bash@0.2.0` owned
by `builtin-tool-owner@golem.cloud`, from its own component (`golem:bash-0-2-0`) in the
`golem-system/builtin-tools` environment. A new version replaces the previous one there and
supersedes its release; grants of the previous release keep working.

Build and validate the embedded artifact from the repository root:

```shell
cargo make build-builtin-tools
```

The task builds the component reproducibly in a pinned container
(`plugins/builtin-tools/bash/build-bash-wasm.sh`), writes it to the path embedded by the registry
service, and validates it with `wasm-tools`.

## Adding a component-implemented built-in tool

1. Add a standalone tool component source and build it through its Golem application manifest. Do
   not invoke its language compiler directly.
2. Add the committed WASM to `BuiltinToolDescriptor`, using `include_bytes!` so startup never
   depends on a filesystem path.
3. Set `component_name`, `tool_name`, and `release_version` to the artifact's exported metadata.
   Provisioning validates these values before writing anything.
4. Add the source directory to `cargo make build-builtin-tools`, validate the resulting WASM, and
   run the registry provisioning tests.
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

For Bash, the release declaration has this shape:

```yaml
tools:
  bash:
    release:
      account: builtin-tool-owner@golem.cloud
      name: bash
      version: 0.1.0

agents:
  MyAgent:
    tools:
      bash: {}
```

Use `bash: { filesystemAccess: allowed }` for shell access to the owner's filesystem. Release
availability does not grant that access. See [Bash's contract and examples](bash/README.md).

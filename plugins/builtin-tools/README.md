# Built-in tools

Built-in tools are tool components shipped as bytes in the registry-service binary and provisioned
at registry startup. `BUILTIN_TOOLS` in
`golem-registry-service/src/services/builtin_tool_provisioner.rs` is the production inventory.

The filesystem tools currently have equivalent Rust and MoonBit candidate implementations for
comparison:

- `plugins/filesystem-tools-rust/` → `plugins/filesystem-tools-rust.wasm`
- `plugins/filesystem-tools-moonbit/` → `plugins/filesystem-tools-moonbit.wasm`

The candidates are temporarily registered as `read-file-rust` / `read-file-moonbit` (and likewise
for write and edit), allowing both to be provisioned in the same built-in environment. Once an
implementation is selected, its three tools will take the final unsuffixed names and the other
candidate will be removed.

## Adding a component-implemented built-in tool

1. Add a standalone tool component source and build it through its Golem application manifest. Do
   not invoke its language compiler directly.
2. Add the committed WASM to `BuiltinToolDescriptor`, using `include_bytes!` so startup never
   depends on a filesystem path.
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

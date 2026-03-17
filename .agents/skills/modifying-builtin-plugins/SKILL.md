---
name: modifying-builtin-plugins
description: Building or modifying built-in plugins in plugins/. Use when changing the OTLP exporter plugin code, rebuilding plugin WASMs, or modifying the builtin plugin provisioning logic.
---

# Modifying Built-in Plugins

Built-in plugins are WASM components that ship with Golem and are automatically provisioned at registry service startup. Currently the only built-in plugin is the **OTLP exporter** oplog processor.

## Plugin Location

```
plugins/
  otlp-exporter/                    # Golem application project (standalone workspace)
    components-rust/otlp-exporter/
      src/lib.rs                    # Plugin entry point
      src/config.rs                 # Configuration parsing
      src/export.rs                 # OTLP HTTP export logic
      src/processing.rs            # Oplog entry processing
      src/state.rs                 # Worker state management
      src/otlp_json.rs             # OTLP JSON types
      src/helpers.rs               # Utility functions
      golem.yaml                   # Component manifest (includes copy command)
    Cargo.toml                      # Workspace Cargo.toml (NOT part of main workspace)
    golem.yaml                      # Application manifest
  otlp-exporter.wasm                # Compiled WASM — COMMITTED TO GIT
```

**Important:** The `plugins/otlp-exporter/` directory is a **standalone Golem application** with its own workspace. It is NOT part of the main Golem Cargo workspace. It depends on the Rust SDK at `sdks/rust/golem-rust` via a relative path.

## Building Plugins

### Using cargo-make (preferred for CI / full builds)

```shell
cargo make build-plugins
```

This requires the `golem` CLI binary to be built first (it depends on `build` task). It runs:
1. `golem build -P release --force-build` in `plugins/otlp-exporter/`
2. `golem exec -P release copy` to copy the output WASM to `plugins/otlp-exporter.wasm`

### Manual build (for iterating on plugin code)

```shell
cd plugins/otlp-exporter
golem build -P release
golem exec -P release copy
```

The `copy` custom command (defined in `components-rust/otlp-exporter/golem.yaml`) copies the release WASM from `golem-temp/agents/otlp_exporter_release.wasm` to `plugins/otlp-exporter.wasm`.

### After rebuilding

The compiled `plugins/otlp-exporter.wasm` **must be committed to git**. It is consumed at compile time by the `golem` CLI binary via `include_bytes!` and at runtime by the test framework.

## How Plugins Are Loaded

### In the `golem` CLI (local mode)

The WASM is embedded at compile time:
```rust
// cli/golem/src/launch.rs
static OTLP_EXPORTER_WASM: &[u8] = include_bytes!("../../../plugins/otlp-exporter.wasm");
```

### In the test framework

The WASM is loaded from the filesystem at runtime:
```rust
// golem-test-framework/src/components/registry_service/spawned.rs
let otlp_wasm = working_directory.join("../plugins/otlp-exporter.wasm");
```

The path is passed to the registry service via the `GOLEM__BUILTIN_PLUGINS__OTLP_EXPORTER_WASM_PATH` environment variable.

### In the registry service

The bootstrap code in `golem-registry-service/src/bootstrap/mod.rs` loads the WASM from the configured path, then calls `provision_builtin_plugins()` which:

1. Creates or finds the `golem-system` application
2. Creates or finds the `builtin-plugins` environment
3. Uploads the WASM as the `otlp-exporter` component
4. Deploys the `builtin-plugins` environment
5. Registers the `golem-otlp-exporter` plugin (version defined in `builtin_plugin_provisioner.rs`)
6. Grants the plugin to all existing environments

## Configuration

The `BuiltinPluginsConfig` struct in `golem-registry-service/src/config.rs`:

```rust
pub struct BuiltinPluginsConfig {
    pub enabled: bool,
    pub otlp_exporter_wasm: Option<Arc<[u8]>>,       // Set programmatically
    pub otlp_exporter_wasm_path: Option<PathBuf>,     // From config/env var
}
```

Environment variables:
- `GOLEM__BUILTIN_PLUGINS__ENABLED` — enable/disable plugin provisioning
- `GOLEM__BUILTIN_PLUGINS__OTLP_EXPORTER_WASM_PATH` — filesystem path to the WASM file

## Plugin Versioning

The plugin name and version are constants in `golem-registry-service/src/services/builtin_plugin_provisioner.rs`:

```rust
const OTLP_PLUGIN_NAME: &str = "golem-otlp-exporter";
const OTLP_PLUGIN_VERSION: &str = "1.5.0";
```

When updating the plugin, bump `OTLP_PLUGIN_VERSION` if the plugin spec changes.

## Common Workflows

### Modifying plugin logic

1. Edit source files in `plugins/otlp-exporter/components-rust/otlp-exporter/src/`
2. Build: `cd plugins/otlp-exporter && golem build -P release && golem exec -P release copy`
3. Verify `plugins/otlp-exporter.wasm` was updated
4. Commit the updated WASM file
5. Rebuild the main project (`cargo make build`) if testing with the embedded CLI

### Changing the plugin SDK dependency

The plugin uses `golem-rust` from `sdks/rust/golem-rust` with the `export_oplog_processor` feature. If the SDK changes:
1. Rebuild the SDK if needed (see `sdk-development` skill)
2. Rebuild the plugin: `cd plugins/otlp-exporter && golem build -P release --force-build && golem exec -P release copy`
3. Commit the updated WASM

### Modifying provisioning logic

The provisioning code is in `golem-registry-service/src/services/builtin_plugin_provisioner.rs`. Changes there only require rebuilding `golem-registry-service`, not the plugin WASM.

### Testing

The OTLP plugin integration test is at `integration-tests/tests/otlp_plugin.rs`. Run it with:
```shell
cargo test -p integration-tests -- otlp_plugin --report-time
```

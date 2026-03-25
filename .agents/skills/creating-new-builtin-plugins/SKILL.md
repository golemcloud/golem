---
name: creating-new-builtin-plugins
description: Adding a new built-in plugin to Golem. Use when creating a brand new plugin that ships with Golem and is auto-provisioned at startup, covering project scaffolding, WASM compilation, provisioning wiring, CLI embedding, and test framework integration.
---

# Creating a New Built-in Plugin

This skill covers how to add a **new** built-in plugin to Golem end-to-end. Built-in plugins are WASM components compiled with the Golem CLI, committed as `.wasm` files, embedded in the `golem` CLI binary, and automatically provisioned by the registry service at startup.

For modifying an **existing** plugin, load the `modifying-builtin-plugins` skill instead.

## Architecture Overview

A built-in plugin has these integration points (all must be wired up):

1. **Plugin source code** — a standalone Golem application under `plugins/`
2. **Compiled WASM artifact** — committed to git at `plugins/<name>.wasm`
3. **Build task** — added to `Makefile.toml` under `build-plugins`
4. **Registry service config** — `BuiltinPluginsConfig` struct with WASM bytes/path fields
5. **Provisioner** — `builtin_plugin_provisioner.rs` creates the component, deploys it, registers the plugin, and grants it
6. **Bootstrap wiring** — `golem-registry-service/src/bootstrap/mod.rs` loads WASM and calls the provisioner
7. **CLI embedding** — `cli/golem/src/launch.rs` uses `include_bytes!` to embed the WASM
8. **Test framework** — `golem-test-framework/src/components/registry_service/` loads WASM from filesystem and passes it via env vars

## Step-by-Step Guide

### 1. Create the Plugin Project

Create a new Golem application under `plugins/`:

```
plugins/
  my-plugin/
    components-rust/my-plugin/
      src/lib.rs                    # Plugin implementation
      Cargo.toml                    # crate-type = ["cdylib"]
      golem.yaml                    # Component manifest with copy command
    Cargo.toml                      # Workspace Cargo.toml
    golem.yaml                      # Application manifest
    .gitignore                      # Ignore target/ and golem-temp/
```

Use the existing `plugins/otlp-exporter/` as a template:

**`plugins/my-plugin/.gitignore`:**
```
target/
golem-temp/
```

**`plugins/my-plugin/Cargo.toml`** (workspace root):
```toml
[workspace]
resolver = "2"
members = ["components-rust/*"]

[profile.release]
opt-level = "s"
lto = true

[workspace.dependencies]
golem-rust = { path = "../../sdks/rust/golem-rust", features = ["export_oplog_processor"] }
# Add other dependencies as needed
```

Note: The `features` on `golem-rust` depend on the plugin type. For oplog processors use `export_oplog_processor`. Adjust based on the plugin kind.

**`plugins/my-plugin/golem.yaml`:**
```yaml
app: my-plugin

includes:
  - components-*/*/golem.yaml

environments:
  local:
    server: local
    componentPresets: debug
  cloud:
    server: cloud
    componentPresets: release
```

**`plugins/my-plugin/components-rust/my-plugin/Cargo.toml`:**
```toml
[package]
name = "my_plugin"
version = "0.0.1"
edition = "2021"

[lib]
crate-type = ["cdylib"]
path = "src/lib.rs"

[dependencies]
golem-rust = { workspace = true }
```

**`plugins/my-plugin/components-rust/my-plugin/golem.yaml`:**
```yaml
components:
  my:plugin:
    templates: rust

customCommands:
  copy:
    - command: cp ../../golem-temp/agents/my_plugin_release.wasm ../../../my-plugin.wasm
```

The copy command filename is derived from the component name: colons become underscores, suffixed with `_release.wasm`. Verify the actual output filename in `golem-temp/agents/` after building.

### 2. Implement the Plugin

Write the plugin code in `plugins/my-plugin/components-rust/my-plugin/src/lib.rs`. The implementation depends on the plugin type (currently only `OplogProcessor` is supported):

```rust
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::Guest as OplogProcessorGuest;
use golem_rust::bindings::golem::api::oplog::{OplogEntry, OplogIndex};
use golem_rust::golem_wasm::golem_core_1_5_x::types::{AgentId, ComponentId};

struct MyPluginComponent;

impl OplogProcessorGuest for MyPluginComponent {
    fn process(
        _account_info: golem_rust::oplog_processor::exports::golem::api::oplog_processor::AccountInfo,
        config: Vec<(String, String)>,
        component_id: ComponentId,
        worker_id: AgentId,
        metadata: golem_rust::bindings::golem::api::host::AgentMetadata,
        _first_entry_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), String> {
        // Plugin logic here
        Ok(())
    }
}

golem_rust::oplog_processor::export_oplog_processor!(MyPluginComponent with_types_in golem_rust::oplog_processor);
```

### 3. Build and Commit the WASM

```shell
cd plugins/my-plugin
golem build -P release
golem exec -P release copy
```

Verify `plugins/my-plugin.wasm` was created, then **commit it to git**. This file is required at compile time by the CLI.

### 4. Add to `Makefile.toml`

Update the `build-plugins` task in `Makefile.toml` to include the new plugin:

```toml
[tasks.build-plugins]
dependencies = ["build"]
description = "Builds built-in plugins (requires golem CLI to be built first)"
script_runner = "@duckscript"
script = '''
cd plugins/otlp-exporter
exec --fail-on-error ../../target/debug/golem build -P release --force-build
exec --fail-on-error ../../target/debug/golem exec -P release copy
cd ../..
cd plugins/my-plugin
exec --fail-on-error ../../target/debug/golem build -P release --force-build
exec --fail-on-error ../../target/debug/golem exec -P release copy
cd ../..
'''
```

### 5. Add Config Fields

In `golem-registry-service/src/config.rs`, add fields to `BuiltinPluginsConfig`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct BuiltinPluginsConfig {
    pub enabled: bool,
    #[serde(skip)]
    pub otlp_exporter_wasm: Option<Arc<[u8]>>,
    pub otlp_exporter_wasm_path: Option<PathBuf>,
    #[serde(skip)]
    pub my_plugin_wasm: Option<Arc<[u8]>>,           // New
    pub my_plugin_wasm_path: Option<PathBuf>,         // New
}
```

The `#[serde(skip)]` field holds the in-memory WASM bytes (set programmatically). The path field is set via environment variable `GOLEM__BUILTIN_PLUGINS__MY_PLUGIN_WASM_PATH`.

### 6. Wire Up Provisioning

In `golem-registry-service/src/services/builtin_plugin_provisioner.rs`, add provisioning logic for the new plugin. The provisioner follows a consistent pattern:

1. **Get or skip** — check if the WASM bytes are provided; skip if not
2. **Create component** — upload WASM into the `builtin-plugins` environment (idempotent: handle `ComponentWithNameAlreadyExists`)
3. **Deploy environment** — call `deployment_write_service.create_deployment()` so the component becomes deployed
4. **Register plugin** — create a `PluginRegistrationCreation` with the appropriate `PluginSpecDto` variant (idempotent: handle `PluginNameAndVersionAlreadyExists`)
5. **Grant to environments** — iterate all environments and grant the plugin

All plugins share the same `golem-system` application and `builtin-plugins` environment. Add new component and plugin constants:

```rust
const MY_PLUGIN_COMPONENT_NAME: &str = "my:plugin";
const MY_PLUGIN_NAME: &str = "golem-my-plugin";
const MY_PLUGIN_VERSION: &str = "1.0.0";
```

### 7. Wire Up Bootstrap

In `golem-registry-service/src/bootstrap/mod.rs`, load the new plugin's WASM from the filesystem path (similar to the OTLP exporter pattern):

```rust
if builtin_plugins.my_plugin_wasm.is_none() {
    if let Some(ref path) = builtin_plugins.my_plugin_wasm_path {
        match std::fs::read(path) {
            Ok(bytes) => {
                tracing::info!("Loaded my-plugin WASM from {}", path.display());
                builtin_plugins.my_plugin_wasm = Some(Arc::from(bytes));
            }
            Err(e) => {
                return Err(anyhow!(
                    "Failed to read my-plugin WASM from {}: {e}",
                    path.display()
                ));
            }
        }
    }
}
```

### 8. Embed in the CLI

In `cli/golem/src/launch.rs`, add an `include_bytes!` for the new WASM and pass it in the config:

```rust
static MY_PLUGIN_WASM: &[u8] = include_bytes!("../../../plugins/my-plugin.wasm");

// In the BuiltinPluginsConfig construction:
builtin_plugins: BuiltinPluginsConfig {
    enabled: true,
    otlp_exporter_wasm: Some(Arc::from(OTLP_EXPORTER_WASM)),
    my_plugin_wasm: Some(Arc::from(MY_PLUGIN_WASM)),
    ..Default::default()
},
```

### 9. Wire Up Test Framework

In `golem-test-framework/src/components/registry_service/`:

**`spawned.rs`** — load the WASM path:
```rust
let my_plugin_wasm = working_directory.join("../plugins/my-plugin.wasm");
let my_plugin_wasm_path = if my_plugin_wasm.exists() {
    Some(my_plugin_wasm.as_path())
} else {
    None
};
```

**`mod.rs`** — pass the path as an env var in `env_vars()`:
```rust
// Add parameter: my_plugin_wasm_path: Option<&Path>
let builder = if let Some(wasm_path) = my_plugin_wasm_path {
    builder.with(
        "GOLEM__BUILTIN_PLUGINS__MY_PLUGIN_WASM_PATH",
        wasm_path.to_string_lossy().to_string(),
    )
} else {
    builder
};
```

## Checklist

- [ ] Plugin source created under `plugins/<name>/`
- [ ] Plugin compiles: `cd plugins/<name> && golem build -P release && golem exec -P release copy`
- [ ] `plugins/<name>.wasm` exists and is committed to git
- [ ] `Makefile.toml` `build-plugins` task updated
- [ ] `BuiltinPluginsConfig` has new WASM and path fields
- [ ] `builtin_plugin_provisioner.rs` provisions the new plugin (create component, deploy, register, grant)
- [ ] `bootstrap/mod.rs` loads WASM from filesystem path
- [ ] `cli/golem/src/launch.rs` embeds WASM via `include_bytes!`
- [ ] Test framework passes WASM path via env var
- [ ] Plugin version constant defined in provisioner
- [ ] Integration test written (optional but recommended)

## Plugin Types

Currently only `OplogProcessor` plugins are supported (see `PluginSpecDto` enum in `golem-common/src/model/plugin_registration.rs`). If adding a new plugin type, you'll also need to extend `PluginSpecDto` and the associated model/repo/service code.

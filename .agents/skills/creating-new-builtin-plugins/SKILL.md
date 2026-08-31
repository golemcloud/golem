---
name: creating-new-builtin-plugins
description: Adds a built-in WASM plugin that is embedded in and provisioned by the registry service. Use when creating a plugin that ships with Golem and is granted to every environment.
---

# Creating a New Built-in Plugin

Use `plugins/otlp-exporter/` and `golem-registry-service/src/services/builtin_plugin_provisioner.rs` as the source of truth. Built-in plugins are standalone Golem applications whose committed WASM is embedded directly in the **registry-service binary**. Embedding keeps provisioning self-contained in every registry-service deployment; the CLI, bootstrap, and test framework do not load plugin paths or bytes.

## Workflow

1. Create `plugins/<plugin>/` as a standalone workspace and Golem application. Follow the OTLP exporter layout and its scoped `AGENTS.md`.
2. Select dependencies from current workspace/plugin manifests rather than copying old versions. For an oplog processor, use the current `golem-rust` path and `export_oplog_processor` feature plus the exact async `wit-bindgen` setup used by the OTLP exporter.
3. Implement the current async SDK interface. The essential shape is:

```rust
use golem_rust::bindings::golem::api::oplog::{OplogEntry, OplogIndex};
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::Guest as OplogProcessorGuest;
use golem_rust::schema::wit::wire::{AgentId, ComponentId};

impl OplogProcessorGuest for MyPluginComponent {
    async fn process(
        _account_info: golem_rust::oplog_processor::exports::golem::api::oplog_processor::AccountInfo,
        config: Vec<(String, String)>,
        component_id: ComponentId,
        worker_id: AgentId,
        metadata: golem_rust::oplog_processor::host::AgentMetadata,
        _first_entry_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), String> {
        todo!()
    }
}

golem_rust::oplog_processor::export_oplog_processor!(MyPluginComponent with_types_in golem_rust::oplog_processor);
```

4. Add a release-profile `copy` custom command that writes `plugins/<plugin>.wasm` from the actual `golem-temp/agents/*_release.wasm` output.
5. Extend `build-plugins` in `Makefile.toml`. Keep it as duckscript, resolve the local binary through `CARGO_MAKE_CRATE_TARGET_DIRECTORY`, and pass `--yes` to every `golem build` invocation. Never hardcode `target/` or use `cargo metadata` in the task.
6. Run `cargo make build-plugins`. Confirm the destination exists, is non-empty, changed when expected, and validates with `wasm-tools validate --features all plugins/<plugin>.wasm`. Commit the WASM because `include_bytes!` requires it while compiling registry service.
7. Add one `BuiltinPluginDescriptor` entry to `BUILTIN_PLUGINS` with component name, plugin name, version, description, and `include_bytes!("../../../plugins/<plugin>.wasm")`. Add descriptor behavior only if the plugin uses a new `PluginSpecDto` variant.

Do **not** add per-plugin fields, bytes, or paths to `BuiltinPluginsConfig`; it only selects `Enabled` or `Disabled`. Do not add bootstrap, CLI embedding, environment variables, or test-framework wiring. Bootstrap calls the descriptor-driven provisioner once.

## Provisioning and Grants

The shared provisioner creates or finds the built-in owner's `golem-system` application and `builtin-plugins` environment, uploads or hash-updates every descriptor component, deploys once, and idempotently registers each plugin. Existing registration by the same name/version is accepted. Existing environments are not iterated during provisioning: `EnvironmentService::create` grants every plugin owned by the built-in-plugin owner transactionally to each new environment, and built-in grants cannot be deleted.

If changing this behavior, update focused service/integration tests. Tests that invoke Cargo,
Golem, or another compiler as a subprocess belong specifically in the CLI integration test suite;
unit, worker-executor, and non-CLI integration tests must not spawn them. Implement the current
contract directly; backward-compatibility paths remain prohibited until the repository-wide policy
is revised.

## Verification

- `cargo make build-plugins`
- `wasm-tools validate --features all plugins/<plugin>.wasm`
- `cargo check -p golem-registry-service`
- `cargo make integration-tests-group7` for built-in plugin provisioning/grant behavior (the authoritative task; it runs `otlp_plugin` and `plugins` serially)
- Inspect `git diff -- plugins Makefile.toml golem-registry-service` and confirm the new WASM is tracked.

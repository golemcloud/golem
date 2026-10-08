---
name: creating-new-builtin-plugins
description: Adds a built-in WASM plugin that is externally released and provisioned by the registry service. Use when creating a plugin that ships with Golem and is granted to every environment.
---

# Creating a New Built-in Plugin

Use `plugins/otlp-exporter/`, `builtin-artifacts.lock.json`, and the registry plugin provisioner as the source of truth. Built-in plugins are standalone Golem applications whose generated WASMs are published as immutable assets in `golemcloud/golem-builtins`. The registry verifies, caches, and provisions those assets at startup.

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
6. Run `cargo make build-plugins`. Confirm the generated destination exists, is non-empty, changed when expected, and validates with `wasm-tools validate --features all plugins/<plugin>.wasm`. Do not commit it.
7. Add one `BuiltinPluginDescriptor` entry with component name, artifact ID, plugin name, version, and description. Add the component name, independent artifact version, and SHA-256 under that artifact ID in `builtin-artifacts.lock.json`.
8. Load `publishing-builtin-artifacts`, publish an independent immutable component version, and verify the pinned URL before merging.

Do **not** add per-plugin fields, bytes, or paths to `BuiltinPluginsConfig`; it only selects `Enabled` or `Disabled`. Do not add `include_bytes!` or commit generated WASMs. Bootstrap calls the descriptor-driven provisioner once through the shared artifact cache.

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
- Run a dry-run publication and inspect `git diff -- plugins builtin-artifacts.lock.json Makefile.toml golem-registry-service`; confirm the generated WASM is ignored.

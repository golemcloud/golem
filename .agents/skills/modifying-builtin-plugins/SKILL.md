---
name: modifying-builtin-plugins
description: Builds and modifies built-in plugins and their descriptor-driven registry-service provisioning. Use for plugin source, committed WASM, descriptors, versions, provisioning, or grants.
---

# Modifying Built-in Plugins

Built-in plugin sources are standalone Golem applications under `plugins/`. Their committed WASMs are embedded by `BUILTIN_PLUGINS` in `golem-registry-service/src/services/builtin_plugin_provisioner.rs`, making registry-service startup independent of filesystem paths and CLI launch mode.

## Source and SDK Changes

Read the plugin's scoped `AGENTS.md`, manifests, and current source before editing. For the OTLP exporter, preserve the current async oplog processor API and imports:

```rust
use golem_rust::bindings::golem::api::oplog::{OplogEntry, OplogIndex};
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::Guest as OplogProcessorGuest;
use golem_rust::schema::wit::wire::{AgentId, ComponentId};
```

`Guest::process` is `async`, metadata is `golem_rust::oplog_processor::host::AgentMetadata`, and the export macro uses `with_types_in golem_rust::oplog_processor`. Find dependency versions/features in current root, SDK, and plugin manifests with targeted `rg`; do not infer them from old examples or add compatibility aliases.

## Build and Validate

Prefer the repository task:

```shell
cargo make build-plugins
wasm-tools validate --features all plugins/otlp-exporter.wasm
```

`build-plugins` uses cargo-make's `CARGO_MAKE_CRATE_TARGET_DIRECTORY`; do not replace it with `target/...` or `cargo metadata`. Every mutating `golem build` command must include `--yes`. Check that the copied WASM exists, is non-empty, validates, and changed when source/SDK inputs changed. Commit it: registry service uses `include_bytes!` at compile time.

## Provisioning Changes

`BuiltinPluginsConfig` is only the tagged `Enabled`/`Disabled` switch. Plugin metadata and embedded bytes belong in `BuiltinPluginDescriptor`/`BUILTIN_PLUGINS`, not config, bootstrap, CLI, environment variables, or test-framework path plumbing.

When enabled, startup creates/finds the built-in owner's system app/environment, hash-updates descriptor components, deploys the environment once, then idempotently registers each descriptor. New environments transactionally receive grants for all plugins owned by the built-in-plugin owner; those grants cannot be deleted. Provisioning does not backfill by iterating existing environments.

Bump the descriptor version when publishing a distinct plugin version. If only provisioning changes, the WASM need not be rebuilt. Keep current behavior direct and remove superseded architecture rather than retaining backwards-compatible branches.

## Verification

- Plugin-only Rust checks/tests from `plugins/<plugin>/` as appropriate
- `cargo make build-plugins` and WASM validation after source, SDK, or manifest changes
- `cargo check -p golem-registry-service` after descriptor/provisioner changes
- `cargo make integration-tests-group7` for authoritative built-in plugin tests

Tests that invoke Cargo, Golem, or a compiler as a subprocess belong specifically in the CLI
integration test suite. Unit tests, worker-executor tests, and non-CLI integration tests must not
spawn them.

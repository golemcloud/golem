---
name: modifying-service-configs
description: "Modifying service configuration types or defaults. Use when changing config structs, adding config fields, or updating default values for any Golem service."
---

# Modifying Service Configs

Golem services use a configuration system built on [Figment](https://github.com/SergioBenitez/Figment) via a custom `ConfigLoader`. Configuration defaults are serialized to TOML and env-var reference files that are checked into the repository and validated in CI.

## How Configuration Works

Each service has a configuration struct that implements:
- `Default` — provides default values
- `Serialize` / `Deserialize` — for TOML and env-var serialization
- `SafeDisplay` — for logging without exposing secrets

Services load config by merging (in order): defaults → TOML file → environment variables.

## Service Config Locations

| Service | Config struct | File |
|---------|--------------|------|
| Worker Executor | `GolemConfig` | `golem-worker-executor/src/services/golem_config.rs` |
| Worker Service | `WorkerServiceConfig` | `golem-worker-service/src/config.rs` |
| Registry Service | `RegistryServiceConfig` | `golem-registry-service/src/config.rs` |
| Shard Manager | `ShardManagerConfig` | `golem-shard-manager/src/shard_manager_config.rs` |
| Compilation Service | `ServerConfig` | `golem-component-compilation-service/src/config.rs` |

The all-in-one `golem` binary has its own merged config that combines multiple service configs.

## Modifying a Config

### Step 1: Edit the config struct

Add, remove, or modify fields in the appropriate config struct. Update the `Default` implementation if default values change.

### Step 2: Regenerate config files

```shell
cargo make generate-configs
```

This builds the service binaries and runs them with `--dump-config-default-toml` and `--dump-config-default-env-var` flags, producing reference files that reflect the current `Default` implementation.

### Step 3: Verify

```shell
cargo make build
```

### Step 4: Check configs match

CI runs `cargo make check-configs` which regenerates configs and diffs them against committed files. If this fails, you forgot to run `cargo make generate-configs`.

## Adding a New Config Field

1. Add the field to the config struct with a `serde` attribute if needed
2. Set its default value in the `Default` impl
3. Run `cargo make generate-configs` to update reference files
4. If the field requires a new environment variable, the env-var mapping is derived automatically from the field path

## Removing a Config Field

1. Remove the field from the struct and `Default` impl
2. Run `cargo make generate-configs`
3. Check for any code that references the removed field

## Nested Config Types

Many config structs compose sub-configs (e.g., `GolemConfig` contains `WorkersServiceConfig`, `BlobStoreServiceConfig`, etc.). When modifying a sub-config type that's shared across services, regenerate configs for all affected services — `cargo make generate-configs` handles this automatically.

## Checklist

1. Config struct modified with appropriate `serde` attributes
2. `Default` implementation updated
3. `cargo make generate-configs` run
4. Generated TOML and env-var files committed
5. `cargo make build` succeeds
6. `cargo make check-configs` passes (CI validation)
7. `cargo make fix` run before PR

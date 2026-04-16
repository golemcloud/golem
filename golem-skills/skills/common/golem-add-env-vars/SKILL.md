---
name: golem-add-env-vars
description: "Defining environment variables for Golem agents. Use when configuring env vars in golem.yaml at the component template, component, agent type, or preset level, passing env vars to individual agent instances via CLI, or using template substitution and merge modes."
---

# Defining Environment Variables for Golem Agents

Environment variables in Golem can be set at multiple levels in the application manifest (`golem.yaml`) and via CLI parameters. They follow a **cascade property system** where values defined at lower (more specific) levels override or merge with values from higher (more general) levels.

## Cascade Hierarchy (Most General → Most Specific)

```
componentTemplates → components → agents → presets
```

Each level can define `env` (a map of key-value pairs) and `envMergeMode` (how to combine with the parent level). More specific levels are applied on top of less specific ones.

## 1. Component Template Level

Define shared environment variables for all components that use a template:

```yaml
componentTemplates:
  my-template:
    env:
      LOG_LEVEL: info
      SERVICE_NAME: my-service
```

All components referencing `my-template` via `templates: [my-template]` inherit these variables.

## 2. Component Level

Define or override environment variables for a specific component:

```yaml
components:
  my-ns:my-component:
    templates: [my-template]
    env:
      DATABASE_URL: postgresql://localhost:5432/mydb
      LOG_LEVEL: debug   # overrides template's LOG_LEVEL
```

## 3. Agent Type Level

Define environment variables for a specific agent type within a component:

```yaml
agents:
  MyAgent:
    env:
      CACHE_TTL: "300"
      FEATURE_FLAG: enabled
```

## 4. Preset Level (Component and Agent)

Both components and agents support **presets** that can add or override environment variables. Presets are selected at build/deploy time.

### Component preset:

```yaml
components:
  my-ns:my-component:
    env:
      LOG_LEVEL: info
    presets:
      debug:
        default: {}
        env:
          LOG_LEVEL: debug
          DEBUG_MODE: "true"
      release:
        env:
          LOG_LEVEL: warn
```

### Agent preset:

```yaml
agents:
  MyAgent:
    env:
      CACHE_TTL: "300"
    presets:
      debug:
        default: {}
        env:
          CACHE_TTL: "60"
```

## Complete Multi-Level Example

```yaml
componentTemplates:
  shared:
    env:
      LOG_LEVEL: info
      REGION: us-east-1

components:
  my-ns:my-component:
    templates: [shared]
    env:
      DATABASE_URL: postgresql://db:5432/app
    presets:
      debug:
        default: {}
        env:
          LOG_LEVEL: debug

agents:
  MyAgent:
    env:
      CACHE_TTL: "300"
      API_KEY: "{{ MY_API_KEY }}"
    presets:
      debug:
        default: {}
        env:
          CACHE_TTL: "60"
```

With the `debug` preset, the final resolved env for `MyAgent` would be:

| Variable | Value | Source |
|---|---|---|
| `REGION` | `us-east-1` | componentTemplates.shared |
| `DATABASE_URL` | `postgresql://db:5432/app` | components.my-ns:my-component |
| `LOG_LEVEL` | `debug` | components.my-ns:my-component.presets.debug |
| `CACHE_TTL` | `60` | agents.MyAgent.presets.debug |
| `API_KEY` | *(resolved from host)* | agents.MyAgent |

## Merge Modes

By default, environment variables from child levels are **upserted** (added or updated) into the parent map. You can change this per level with `envMergeMode`:

| Mode | Behavior |
|---|---|
| `upsert` | **(default)** Add new keys, overwrite existing ones |
| `replace` | Discard all parent env vars, use only this level's values |
| `remove` | Remove the listed keys from the parent map |

### Example: Replace all inherited env vars

```yaml
agents:
  MyAgent:
    envMergeMode: replace
    env:
      ONLY_THIS: "true"
```

### Example: Remove specific inherited env vars

```yaml
agents:
  MyAgent:
    envMergeMode: remove
    env:
      LOG_LEVEL: ""   # value is ignored, key is removed
```

## Template Substitution in Values

Environment variable values support **Jinja-style template substitution** using `{{ VAR_NAME }}`. At deploy time, these are resolved against the **host machine's environment variables** (the shell running `golem deploy`):

```yaml
agents:
  MyAgent:
    env:
      API_KEY: "{{ MY_API_KEY }}"
      DB_PASSWORD: "{{ DB_PASSWORD }}"
      MIXED: "prefix-{{ SOME_VAR }}-suffix"
```

If a referenced host variable is missing, deployment fails with a clear error listing the missing variables. The substitution engine uses strict mode — all referenced variables must be defined.

## Reading Environment Variables in Agent Code

**Rust and TypeScript** agents use the standard APIs to read environment variables (`std::env::var` in Rust, `process.env` in TypeScript).

**Scala** agents must use `golem.wasi.Environment.getEnvironment()` which returns a `Map[String, String]` of all environment variables via the WASI `wasi:cli/environment@0.2.3` interface. Standard Scala `sys.env` does **not** work inside the WASM runtime. Example:

```scala
import golem.wasi.Environment

val env = Environment.getEnvironment()
val appMode = env.getOrElse("APP_MODE", "default")
```

## Passing Env Vars to Individual Agent Instances via CLI

When creating an agent instance directly (outside `golem deploy`), you can pass environment variables with the `--env` / `-e` flag:

```shell
golem agent new my-ns:my-component/my-agent-1 \
  --env API_KEY=secret123 \
  --env LOG_LEVEL=debug
```

Multiple `--env` flags can be provided, each in `KEY=VALUE` format. These are set only on that specific agent instance and do not affect the manifest or other instances.

## Summary of All Methods

| Method | Scope | Where |
|---|---|---|
| `componentTemplates.*.env` | All components using the template | `golem.yaml` |
| `components.*.env` | Single component, all its agents | `golem.yaml` |
| `agents.*.env` | Single agent type | `golem.yaml` |
| `*.presets.*.env` | Component or agent preset | `golem.yaml` |
| `envMergeMode` | Controls merge at any level | `golem.yaml` |
| `{{ VAR }}` substitution | Any env value | `golem.yaml` |
| `golem agent new --env KEY=VAL` | Single agent instance | CLI |

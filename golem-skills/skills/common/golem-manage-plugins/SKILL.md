---
name: golem-manage-plugins
description: "Managing Golem plugins — listing available plugins, installing and configuring plugins via golem.yaml or CLI, and understanding built-in plugins like the OTLP exporter."
---

# Managing Golem Plugins

Plugins extend component and agent behavior without modifying application code. Currently, the only plugin type is **Oplog Processor** — a WASM component that receives and processes the operation log entries produced by agents (e.g., exporting traces, logs, or metrics).

## Built-in Plugins

Golem ships with the following built-in plugins, automatically registered and available in every environment:

| Plugin Name | Type | Description |
|-------------|------|-------------|
| `golem-otlp-exporter` | Oplog Processor | Exports agent telemetry (traces, logs, metrics) to any OTLP-compatible collector (Jaeger, Grafana, Datadog, etc.) |

### golem-otlp-exporter Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `endpoint` | Yes | OTLP collector endpoint URL (must start with `http://` or `https://`) |
| `headers` | No | Comma-separated `key=value` pairs sent as HTTP headers (e.g., `x-api-key=secret,auth=token`) |
| `signals` | No | Comma-separated telemetry types to export: `traces`, `logs`, `metrics`. Default: `traces` |
| `service-name-mode` | No | How to set the `service.name` attribute: `agent-id` (default) uses the worker ID, `agent-type` uses the component ID |

## Installing Plugins via golem.yaml

Add plugins to a component or agent in `golem.yaml` using the `plugins` field:

```yaml
components:
  my-app:service:
    plugins:
      - name: golem-otlp-exporter
        version: "1.1.5"
        parameters:
          endpoint: "http://localhost:4318"
          signals: "traces,logs,metrics"

agents:
  MyAgent:
    plugins:
      - name: golem-otlp-exporter
        version: "1.1.5"
        parameters:
          endpoint: "https://otel-collector.example.com:4318"
          headers: "x-api-key=my-secret-key"
          signals: "traces,logs"
          service-name-mode: "agent-type"
```

### Plugin Installation Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Plugin name (e.g., `golem-otlp-exporter`) |
| `version` | Yes | Plugin version string |
| `account` | No | Account that owns the plugin (omit for built-in plugins) |
| `parameters` | No | Key-value map of plugin-specific configuration |

### Using Templates

Plugins can be defined in `componentTemplates` and inherited via the cascade system:

```yaml
componentTemplates:
  observability:
    plugins:
      - name: golem-otlp-exporter
        version: "1.1.5"
        parameters:
          endpoint: "http://localhost:4318"
          signals: "traces,logs,metrics"

components:
  my-app:service:
    templates: [rust, observability]
```

### Plugin Merge Modes

When plugins are inherited from templates, the `pluginsMergeMode` field controls how they combine:

| Mode | Behavior |
|------|----------|
| `append` (default) | Add new plugins after inherited ones |
| `prepend` | Add new plugins before inherited ones |
| `replace` | Discard inherited plugins, use only the ones defined here |

```yaml
components:
  my-app:service:
    templates: [observability]
    pluginsMergeMode: replace
    plugins: []                    # Remove all inherited plugins
```

### Per-environment Plugin Configuration

Use presets and environments to vary plugin parameters across deployment targets:

```yaml
components:
  my-app:service:
    plugins:
      - name: golem-otlp-exporter
        version: "1.1.5"
        parameters:
          endpoint: "http://localhost:4318"
    presets:
      production:
        pluginsMergeMode: replace
        plugins:
          - name: golem-otlp-exporter
            version: "1.1.5"
            parameters:
              endpoint: "https://otel.prod.example.com:4318"
              headers: "x-api-key=${OTLP_API_KEY}"
              signals: "traces,logs,metrics"

environments:
  local:
    server: local
    componentPresets: debug
  production:
    server: cloud
    componentPresets: production
```

## Managing Plugins via CLI

### Listing Available Plugins

```shell
golem plugin list                     # List all registered plugins
```

### Installing a Plugin on a Component (imperative)

```shell
golem component plugin install \
  --component-name my-app:service \
  --plugin-name golem-otlp-exporter \
  --plugin-version "1.1.5" \
  --priority 0 \
  --param endpoint=http://localhost:4318 \
  --param signals=traces,logs
```

### Viewing Installed Plugins

```shell
golem component plugin get \
  --component-name my-app:service
```

### Updating a Plugin

```shell
golem component plugin update \
  --component-name my-app:service \
  --plugin-to-update 0 \
  --priority 1 \
  --param endpoint=https://new-endpoint:4318
```

### Uninstalling a Plugin

```shell
golem component plugin uninstall \
  --component-name my-app:service \
  --plugin-to-update 0
```

## Declarative vs Imperative

- **Declarative (golem.yaml)**: Preferred for repeatable setups. Plugins are installed/updated on `golem deploy`. Configuration lives in version control.
- **Imperative (CLI)**: Useful for quick one-off installations, debugging, or environments where the manifest is not available.

When using `golem deploy`, the manifest is the source of truth — any plugins defined in `golem.yaml` are reconciled with the deployed state.

## Plugin Priority

When multiple plugins are installed, `priority` determines their execution order. Plugins with **higher priority values are applied first**. Priority is set explicitly via the CLI's `--priority` flag; in `golem.yaml`, the order in the `plugins` list determines priority (first entry = highest priority).

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Full docs: https://learn.golem.cloud

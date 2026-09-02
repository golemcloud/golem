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

### Template Substitution in Plugin Parameters

Plugin parameter values support **Jinja-style template substitution** using `{{ VAR_NAME }}`. At deploy time, these are resolved against the **host machine's environment variables**:

```yaml
plugins:
  - name: golem-otlp-exporter
    version: "1.1.5"
    parameters:
      endpoint: "{{ OTLP_ENDPOINT }}"
      headers: "x-api-key={{ OTLP_API_KEY }}"
```

If a referenced variable is missing, deployment fails with the list of unresolved variables. See the `golem-add-env-vars` skill for full details on the substitution syntax.

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

CLI outputs mask plugin parameter values by sensitive-looking parameter names. If a plugin parameter contains a secret, use a name containing words such as `secret`, `token`, `password`, or `key` so commands like `component get`, `component manifest-trace`, and `deploy` mask it by default.

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
              headers: "x-api-key={{ OTLP_API_KEY }}"
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
golem plugin list
golem plugin list --account owner@example.com
golem plugin list --account-id 2f6b30d9-bac2-4c67-9d4f-12ea89ba2211
```

With no account option, the list includes plugins owned by the authenticated account and plugins granted to the selected environment. An explicit account lists plugins owned by that account. `--account` and `--account-id` conflict.

### Inspecting and unregistering registry plugins

```shell
golem plugin get my-plugin 1.0.0
golem plugin get --id 8fd5e4a2-9cab-4f8e-9d3a-1c2e4f567890
golem plugin unregister my-plugin 1.0.0
golem plugin unregister --id 8fd5e4a2-9cab-4f8e-9d3a-1c2e4f567890
```

The name and version form requires both positional values and accepts `--account` or `--account-id`. The `--id` form conflicts with the positional identity and account scope. A positional UUID is a name, not an ID; use `--id` explicitly.

Register a plugin from a JSON manifest, optionally for an explicit account:

```shell
golem plugin register ./my-plugin.json
golem plugin register ./my-plugin.json --account owner@example.com
```

Plugin installation is declarative. The retired `component plugin` and `project plugin` workflows are not available. Define installations in `golem.yaml`; `golem deploy` reconciles the manifest with deployed state.

## Plugin Priority

When multiple plugins are installed, the order in the manifest's `plugins` list determines priority (first entry = highest priority).

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Full docs: https://learn.golem.cloud

## Related Skills

- For enabling the built-in `golem-otlp-exporter` plugin and exporting telemetry from agents, load the language-specific skill: `golem-enable-otlp-rust`, `golem-enable-otlp-ts`, `golem-enable-otlp-scala`, or `golem-enable-otlp-moonbit`

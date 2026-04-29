---
name: golem-enable-otlp-scala
description: "Enabling the OpenTelemetry (OTLP) plugin for a Scala Golem agent — exporting traces, logs, and metrics to an OTLP collector, adding custom spans with the invocation context API."
---

# Enabling OpenTelemetry for a Scala Agent

The `golem-otlp-exporter` is a built-in plugin that exports agent telemetry (traces, logs, metrics) to any OTLP-compatible collector via OTLP/HTTP. No plugin installation is needed — just enable it in the application manifest.

## Step 1 — Enable the Plugin in golem.yaml

Add the plugin to the component (or agent) that should emit telemetry:

```yaml
components:
  my-app:service:
    plugins:
      - name: golem-otlp-exporter
        version: "1.5.0"
        parameters:
          endpoint: "http://localhost:4318"
          signals: "traces,logs,metrics"
```

### Plugin Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `endpoint` | Yes | OTLP collector base URL (e.g., `http://localhost:4318`) |
| `signals` | No | Comma-separated: `traces`, `logs`, `metrics`. Default: `traces` |
| `headers` | No | Comma-separated `key=value` HTTP headers (e.g., `x-api-key=secret`) |
| `service-name-mode` | No | `agent-id` (default) or `agent-type` |

## Step 2 — Deploy

```shell
golem deploy
```

After deployment, newly created agents from this component automatically send telemetry to the configured collector.

## What Gets Exported

### Traces

Spans are created automatically for:
- Agent invocations
- RPC calls to other agents
- Outgoing HTTP requests

Trace and span IDs propagate from inbound HTTP requests (via code-first routes) and are included in outgoing HTTP request headers automatically.

### Custom Spans

Use the `ContextApi` from the Scala SDK to create custom spans:

```scala
import golem.host.ContextApi

val span = ContextApi.startSpan("my-operation")
span.setAttribute("env", ContextApi.AttributeValue.StringValue("production"))
span.setAttributes(List(
  ContextApi.Attribute("service", ContextApi.AttributeValue.StringValue("my-service")),
  ContextApi.Attribute("version", ContextApi.AttributeValue.StringValue("1.0")),
))

// ... do work ...

val ctx = ContextApi.currentContext()
println(s"trace_id: ${ctx.traceId()}")
span.finish()
```

### Logs

When `logs` is included in `signals`, all log output is forwarded to the OTLP collector. See the `golem-logging-scala` skill for full logging guidance.

```scala
println("Hello from Scala!")
Logging.log(LogLevel.Debug, "", "This is a debug log entry")
```

### Metrics

When `metrics` is included in `signals`, the following metrics are exported:

| Metric | Type | Description |
|--------|------|-------------|
| `golem_invocation_count` | Counter | Number of agent method invocations |
| `golem_invocation_duration_ns` | Counter | Invocation duration |
| `golem_invocation_fuel_consumed` | Counter | Fuel consumed by invocations |
| `golem_invocation_pending_count` | Counter | Number of pending invocations |
| `golem_host_call_count` | Counter | Number of internal host calls |
| `golem_log_count` | Counter | Number of log entries emitted |
| `golem_memory_initial_bytes` | Gauge | Initially allocated memory |
| `golem_memory_total_bytes` | Gauge | Total allocated memory |
| `golem_memory_growth_bytes` | Counter | Memory growth since start |
| `golem_component_size_bytes` | Gauge | Component size in bytes |
| `golem_error_count` | Counter | Number of recorded errors |
| `golem_interruption_count` | Counter | Number of interrupt requests |
| `golem_exit_count` | Counter | Number of process exit signals |
| `golem_restart_count` | Counter | Number of times a fresh state was created |
| `golem_resources_created` | Counter | Number of internal resources created |
| `golem_resources_dropped` | Counter | Number of internal resources dropped |
| `golem_resources_active` | Gauge | Number of active internal resources |
| `golem_update_success_count` | Counter | Number of successful updates |
| `golem_update_failure_count` | Counter | Number of failed updates |
| `golem_transaction_committed` | Counter | Number of committed database transactions |
| `golem_transaction_rolled_back` | Counter | Number of rolled back database transactions |
| `golem_snapshot_size_bytes` | Counter | Snapshot size in bytes |
| `golem_oplog_processor_lag` | Gauge | Oplog processor delivery lag |

Each metric includes `service.name`, `golem.agent.id`, `golem.component.id`, and `golem.component.version` attributes.

## Local Observability Stack

The Golem repository includes a ready-made Docker Compose setup at `docker-examples/otlp-collector/`:

```shell
docker compose -f docker-examples/otlp-collector/docker-compose.yml up -d
```

This starts:
- **OTel Collector** on port 4318 (OTLP/HTTP)
- **Jaeger** on http://localhost:16686 (traces)
- **Prometheus** on http://localhost:9090 (metrics)
- **Loki** via Grafana (logs)
- **Grafana** on http://localhost:3000 (admin/admin)

Configure the plugin with `endpoint: "http://localhost:4318"` to use this stack.

## Per-Environment Configuration

Use presets to vary the endpoint across environments:

```yaml
components:
  my-app:service:
    plugins:
      - name: golem-otlp-exporter
        version: "1.5.0"
        parameters:
          endpoint: "http://localhost:4318"
          signals: "traces,logs,metrics"
    presets:
      production:
        pluginsMergeMode: replace
        plugins:
          - name: golem-otlp-exporter
            version: "1.5.0"
            parameters:
              endpoint: "https://otel.prod.example.com:4318"
              headers: "x-api-key=${OTLP_API_KEY}"
              signals: "traces,logs,metrics"
```

## Key Points

- **Built-in** — no `golem plugin register` needed, just add to `golem.yaml`
- **Deploy required** — run `golem deploy` after adding the plugin configuration
- Trace context propagates automatically through HTTP routes and RPC calls
- Use `ContextApi.startSpan` from `golem.host.ContextApi` for custom spans
- Plugin can be activated/deactivated per agent with `golem agent activate-plugin` / `golem agent deactivate-plugin`

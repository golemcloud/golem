---
name: golem-enable-otlp-rust
description: "Enabling the OpenTelemetry (OTLP) plugin for a Rust Golem agent — exporting traces, logs, and metrics to an OTLP collector, adding custom spans with the invocation context API."
---

# Enabling OpenTelemetry for a Rust Agent

The `golem-otlp-exporter` is a built-in plugin that exports agent telemetry (traces, logs, metrics) to any OTLP-compatible collector via OTLP/HTTP. No plugin installation is needed — just enable it in the application manifest.

## Step 1 — Enable the Plugin in golem.yaml

Add the plugin to the component (or agent) that should emit telemetry:

```yaml
components:
  my-app:service:
    plugins:
      - name: golem-otlp-exporter
        version: "1.5.8"
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
golem deploy --yes
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

Use the `golem::api::context` bindings to create custom spans:

```rust
use golem_rust::bindings::golem::api::context;

let span = context::start_span("my-operation");
span.set_attribute("env", &context::AttributeValue::String("production".to_string()));
span.set_attributes(&[
    context::Attribute { key: "service".to_string(), value: context::AttributeValue::String("my-service".to_string()) },
    context::Attribute { key: "version".to_string(), value: context::AttributeValue::String("1.0".to_string()) },
]);

// ... do work ...

let ctx = context::current_context();
println!("trace_id: {}", ctx.trace_id());
span.finish();
```

### Logs

When `logs` is included in `signals`, all log output is forwarded to the OTLP collector. See the `golem-logging-rust` skill for full logging guidance.

```rust
use log::debug;

println!("Hello from Rust!");
debug!("This is a debug log entry");
```

### Metrics

When `metrics` is included in `signals`, exported metrics include the following non-exhaustive list:

| Metric | Type | Description |
|--------|------|-------------|
| `golem.invocation.count` | Counter | Number of agent method invocations |
| `golem.invocation.duration_ns` | Counter | Invocation duration |
| `golem.invocation.fuel_consumed` | Counter | Fuel consumed by invocations |
| `golem.invocation.pending_count` | Counter | Number of invocation requests queued (not current queue depth) |
| `golem.host_call.count` | Counter | Number of internal host calls |
| `golem.log.count` | Counter | Number of log entries emitted |
| `golem.memory.initial_bytes` | Gauge | Initially allocated memory |
| `golem.memory.total_bytes` | Gauge | Total allocated memory |
| `golem.memory.growth_bytes` | Counter | Memory growth since start |
| `golem.component.size_bytes` | Gauge | Component size in bytes |
| `golem.error.count` | Counter | Number of recorded errors |
| `golem.interruption.count` | Counter | Number of interrupt requests |
| `golem.exit.count` | Counter | Number of process exit signals |
| `golem.restart.count` | Counter | Number of times a fresh state was created |
| `golem.resources.created` | Counter | Number of internal resources created |
| `golem.resources.dropped` | Counter | Number of internal resources dropped |
| `golem.resources.active` | Gauge | Number of active internal resources |
| `golem.update.success_count` | Counter | Number of successful updates |
| `golem.update.failure_count` | Counter | Number of failed updates |
| `golem.transaction.committed` | Counter | Number of committed remote transaction events |
| `golem.transaction.rolled_back` | Counter | Number of rolled back remote transaction events |
| `golem.snapshot.size_bytes` | Counter | Snapshot size in bytes |
| `golem.oplog_processor.lag` | Gauge | Oplog processor delivery lag |

These are emitted OTLP names. A backend such as Prometheus may normalize dots to underscores and add suffixes; use that backend's mapping when writing queries.

Each metric includes `service.name`, `golem.agent.id`, `golem.component.id`, and `golem.component.version` attributes.

## Export Semantics

- Durable `Start`, `End`, and `Cancelled` metadata drives span lifecycle. Long-lived spans retain their origin trace across invocations; failed or retrying attempts do not prematurely close the logical span. Logs use their recorded trace context.
- Stream summaries use bounded event/outcome metric labels and aggregate item counts; they do not emit a span per item. Active resource and memory values are gauges, and metric labels avoid resource IDs and payload values.
- In `agent-type` service-name mode, `service.name` excludes constructor parameters and phantom instance IDs; `golem.agent.id` remains the full identity.
- Collector export is best effort. Accepted source state advances when a send fails, and traces, logs, and metrics are still attempted independently. Exactly-once plugin batch delivery is not exactly-once collector delivery.
- Fork/revert cannot retract telemetry already accepted by a collector. Source deletion does not provide a terminal signal, so no successful close is invented. Switching plugin instances can lose open-span state; continuity is deferred to GOL-667.

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

Use the environment selector `app-env:production`. A plain `production` custom preset can become
the default custom preset when it is the only one, applying the production endpoint in every environment.

```yaml
components:
  my-app:service:
    plugins:
      - name: golem-otlp-exporter
        version: "1.5.8"
        parameters:
          endpoint: "http://localhost:4318"
          signals: "traces,logs,metrics"
    presets:
      app-env:production:
        pluginsMergeMode: replace
        plugins:
          - name: golem-otlp-exporter
            version: "1.5.8"
            parameters:
              endpoint: "https://otel.prod.example.com:4318"
              headers: "x-api-key={{ OTLP_API_KEY }}"
              signals: "traces,logs,metrics"
```

## Key Points

- **Built-in** — no `golem plugin register` needed, just add to `golem.yaml`
- **Deploy required** — run `golem deploy` after adding the plugin configuration
- Trace context propagates automatically through HTTP routes and RPC calls
- Use `context::start_span` from `golem_rust::bindings::golem::api::context` for custom spans
- Plugin can be activated/deactivated per agent with `golem agent activate-plugin` / `golem agent deactivate-plugin`

## Related Skills

- Load `golem-manage-plugins` for the general plugin installation model (manifest sections, CLI commands, priority, per-environment configuration)

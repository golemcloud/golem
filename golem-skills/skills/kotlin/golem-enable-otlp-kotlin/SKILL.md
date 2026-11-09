---
name: golem-enable-otlp-kotlin
description: "Enabling the OpenTelemetry (OTLP) plugin for a Kotlin Golem agent — exporting traces, logs, and metrics to an OTLP collector, adding custom spans with the invocation context API."
---

# Enabling OpenTelemetry for a Kotlin Agent

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
golem deploy --yes
```

After deployment, newly created agents from this component automatically send telemetry to the configured collector.

## What Gets Exported

### Traces

Spans are created automatically for agent invocations and RPC calls to other agents. Trace and span IDs propagate from inbound HTTP requests and are included in outgoing HTTP request headers automatically.

### Custom Spans

Use `cloud.golem.runtime.host.ContextApi` to create custom spans:

```kotlin
import cloud.golem.runtime.host.AttributeValue
import cloud.golem.runtime.host.Attribute
import cloud.golem.runtime.host.ContextApi

val span = ContextApi.startSpan("my-operation")
span.setAttribute("env", AttributeValue.StringValue("production"))
span.setAttributes(listOf(
    Attribute("service", AttributeValue.StringValue("my-service")),
    Attribute("version", AttributeValue.StringValue("1.0")),
))

// ... do work ...

val ctx = ContextApi.currentContext()
println("trace_id: ${ctx.traceId()}")
span.finish()
```

- `ContextApi.currentContext(): InvocationContext` also exposes `spanId()`, `parent()`, `getAttribute(key, inherited)`, `getAttributeChain(key)` and `traceContextHeaders()` (the `(name, value)` header pairs to propagate trace context into an outgoing call).
- `ContextApi.allowForwardingTraceContextHeaders(allow: Boolean): Boolean` controls whether inbound trace headers are echoed into outgoing ones; it returns the previous setting.
- `AttributeValue` currently has one case, `AttributeValue.StringValue`.
- `Span.close()` and `InvocationContext.close()` release the underlying host resources; `finish()` ends the span's duration (call `close()` after).

### Logs

When `logs` is included in `signals`, all log output is forwarded to the OTLP collector. See `golem-logging-kotlin` for logging guidance.

### Metrics

When `metrics` is included in `signals`, Golem exports a fixed set of invocation, memory, resource and transaction counters and gauges (`golem_invocation_count`, `golem_memory_total_bytes`, `golem_resources_active`, `golem_transaction_committed`, ...), each tagged with `service.name`, `golem.agent.id`, `golem.component.id` and `golem.component.version`. This set is defined by the host plugin and is the same regardless of the agent's source language.

## Export Semantics

- Durable `Start`, `End`, and `Cancelled` metadata drives span lifecycle. Long-lived spans retain their origin trace across invocations; failed or retrying attempts do not prematurely close the logical span.
- Stream summaries use bounded event/outcome metric labels and aggregate item counts; they do not emit a span per item.
- In `agent-type` service-name mode, `service.name` excludes constructor parameters and phantom instance IDs; `golem.agent.id` remains the full identity.
- Collector export is best effort: accepted source state advances even when a send fails. Fork/revert cannot retract telemetry already accepted by a collector.

## Local Observability Stack

The Golem repository includes a ready-made Docker Compose setup at `docker-examples/otlp-collector/`:

```shell
docker compose -f docker-examples/otlp-collector/docker-compose.yml up -d
```

This starts an OTel Collector (port 4318), Jaeger (`http://localhost:16686`), Prometheus (`http://localhost:9090`) and Grafana (`http://localhost:3000`, admin/admin). Configure the plugin with `endpoint: "http://localhost:4318"` to use this stack.

## Key Points

- **Built-in** — no `golem plugin register` needed, just add to `golem.yaml`
- **Deploy required** — run `golem deploy` after adding the plugin configuration
- Trace context propagates automatically through HTTP routes and RPC calls
- Use `ContextApi.startSpan` for custom spans
- The plugin can be activated/deactivated per agent with `golem agent activate-plugin` / `golem agent deactivate-plugin`

## Related Skills

- `golem-logging-kotlin`
- Load `golem-manage-plugins` for the general plugin installation model (manifest sections, CLI commands, priority, per-environment configuration)

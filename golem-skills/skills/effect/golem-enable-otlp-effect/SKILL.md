---
name: golem-enable-otlp-effect
description: "Enabling the built-in OpenTelemetry (OTLP) exporter for Effect-based Golem agents. Use when exporting traces, logs, and metrics or adding Effect spans and structured log annotations."
---

# Enabling OpenTelemetry for an Effect Agent

The `golem-otlp-exporter` is a built-in Golem plugin that exports agent telemetry to an
OTLP-compatible collector over OTLP/HTTP. Enable the plugin in `golem.yaml`; do not add a
JavaScript OpenTelemetry exporter to the component.

`@golemcloud/effect-golem` automatically connects Effect's logger and tracer to the Golem host.
Consequently, normal `Effect.log*`, `Effect.withSpan`, and `Effect.annotateCurrentSpan` calls are
captured by the plugin without an application-provided logging or tracing Layer.

## Enable the Plugin

Add the plugin to the Effect component that should emit telemetry:

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

| Parameter           | Required | Description                                                              |
| ------------------- | -------- | ------------------------------------------------------------------------ |
| `endpoint`          | Yes      | OTLP collector base URL, such as `http://localhost:4318`                 |
| `signals`           | No       | Comma-separated `traces`, `logs`, and/or `metrics`; defaults to `traces` |
| `headers`           | No       | Comma-separated `key=value` HTTP headers, such as `x-api-key=secret`     |
| `service-name-mode` | No       | `agent-id` (default) or `agent-type`                                     |

Deploy the updated application:

```shell
golem deploy --yes
```

New agents created from that component use the configured exporter. Existing plugin activation
can also be managed per agent with `golem agent activate-plugin` and
`golem agent deactivate-plugin`.

## Add Effect Spans and Logs

Use Effect v4 APIs rather than mechanically translating the TypeScript SDK's
`golem:api/context` span handles. `Effect.withSpan` scopes the host span around the Effect and
finishes it on success, failure, or interruption. Initial attributes belong in the `withSpan`
options; add attributes discovered during execution with `Effect.annotateCurrentSpan`.

```typescript
import { Effect, Schema } from "effect";
import { defineAgent, method } from "@golemcloud/effect-golem";

export const TracedAgent = defineAgent({
  name: "TracedAgent",
  mode: "durable",
  constructorParams: {
    instanceName: Schema.String,
  },
  methods: {
    doTracedWork: method({
      params: { taskName: Schema.String },
      success: Schema.String,
    }),
  },
}).implement(({ instanceName }) =>
  Effect.succeed({
    doTracedWork: ({ taskName }) =>
      Effect.gen(function* () {
        yield* Effect.logInfo(`processing: ${taskName}`).pipe(
          Effect.annotateLogs({ instanceName, taskName }),
        );

        return "traced";
      }).pipe(
        Effect.withSpan("process-task", {
          attributes: { task: taskName },
        }),
      ),
  }),
);
```

Import the implemented agent module from `src/main.ts` so its top-level registration runs:

```typescript
import "./traced-agent.js";
```

When an attribute is only known after the span starts, annotate the current span inside its
scoped Effect:

```typescript
const processTask = Effect.gen(function* () {
  const queue = "priority";
  yield* Effect.annotateCurrentSpan({ queue, retryable: true });
  yield* Effect.logInfo("task accepted").pipe(Effect.annotateLogs({ queue }));
}).pipe(Effect.withSpan("process-task"));
```

The SDK converts Effect span attribute values to the string-valued attributes supported by
`golem:api/context@1.5.0`. Effect log annotations and log spans are rendered with the active host
trace and span IDs and emitted through `wasi:logging`; including `logs` in the plugin's `signals`
forwards them to the collector.

## What Gets Exported

### Traces

Golem creates invocation and host-operation spans automatically. The Effect SDK chains
`Effect.withSpan` spans under the invocation's host span, and Golem propagates trace context for
supported inbound HTTP and RPC paths. Failed scoped Effects mark their host spans as errors.

### Logs

Use Effect's logging APIs inside handlers:

```typescript
const logRequest = Effect.gen(function* () {
  yield* Effect.logInfo("request received");
  yield* Effect.logDebug("cache lookup").pipe(
    Effect.annotateLogs({ cacheKey: "item-42" }),
    Effect.withLogSpan("lookup"),
  );
});
```

Prefer these over the lower-level `Logging.log(...)` SDK API so log annotations and Effect log
spans are retained.

### Metrics

When `metrics` is included in `signals`, the exporter sends Golem runtime metrics including:

| Metric                           | Type    | Description                       |
| -------------------------------- | ------- | --------------------------------- |
| `golem_invocation_count`         | Counter | Agent method invocations          |
| `golem_invocation_duration_ns`   | Counter | Invocation duration               |
| `golem_invocation_fuel_consumed` | Counter | Fuel consumed by invocations      |
| `golem_invocation_pending_count` | Counter | Pending invocations               |
| `golem_host_call_count`          | Counter | Internal host calls               |
| `golem_log_count`                | Counter | Emitted log entries               |
| `golem_memory_initial_bytes`     | Gauge   | Initially allocated memory        |
| `golem_memory_total_bytes`       | Gauge   | Total allocated memory            |
| `golem_memory_growth_bytes`      | Counter | Memory growth since start         |
| `golem_component_size_bytes`     | Gauge   | Component size                    |
| `golem_error_count`              | Counter | Recorded errors                   |
| `golem_interruption_count`       | Counter | Interrupt requests                |
| `golem_exit_count`               | Counter | Process exit signals              |
| `golem_restart_count`            | Counter | Fresh state creations             |
| `golem_resources_created`        | Counter | Internal resources created        |
| `golem_resources_dropped`        | Counter | Internal resources dropped        |
| `golem_resources_active`         | Gauge   | Active internal resources         |
| `golem_update_success_count`     | Counter | Successful updates                |
| `golem_update_failure_count`     | Counter | Failed updates                    |
| `golem_transaction_committed`    | Counter | Committed database transactions   |
| `golem_transaction_rolled_back`  | Counter | Rolled-back database transactions |
| `golem_snapshot_size_bytes`      | Counter | Snapshot size                     |
| `golem_oplog_processor_lag`      | Gauge   | Oplog processor delivery lag      |

Metrics include `service.name`, `golem.agent.id`, `golem.component.id`, and
`golem.component.version` attributes.

## Local Observability Stack

The Golem repository includes an OTLP Collector, Jaeger, Prometheus, Loki, and Grafana setup:

```shell
docker compose -f docker-examples/otlp-collector/docker-compose.yml up -d
```

It exposes OTLP/HTTP on port 4318, Jaeger on port 16686, Prometheus on port 9090, and Grafana on
port 3000. Point the plugin's `endpoint` at `http://localhost:4318`.

## Per-Environment Configuration

Use presets when collector configuration differs by environment:

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
              headers: "x-api-key={{ OTLP_API_KEY }}"
              signals: "traces,logs,metrics"
```

## Key Constraints

- Keep OTLP exporter setup in `golem.yaml`; `@golemcloud/effect-golem` has no OTLP or plugin
  installation helper.
- Use `Effect.withSpan` and `Effect.annotateCurrentSpan`; there is no public
  `Tracing.startSpan(...)` wrapper.
- Use `Effect.logInfo`, `Effect.logDebug`, and `Effect.annotateLogs`; do not invent
  `Logging.info(...)` or another SDK logger.
- Do not manually finish an Effect span. Its scoped lifetime is managed by `Effect.withSpan`.
- Do not provide `Logging.layer` or `Tracing.layer` in an agent. The SDK dispatcher installs both.
- Do not add `@effect/opentelemetry`, a Node OpenTelemetry SDK, or a second OTLP exporter inside
  the QuickJS WebAssembly component.
- Keep the generated `effect` and `@golemcloud/effect-golem` versions aligned, and do not edit
  files under `golem-temp/`.

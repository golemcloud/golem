# Local OTLP Collector Stack

A simple Docker Compose stack for trying out Golem's built-in OTLP exporter plugin.
Captures all three OpenTelemetry signals: **traces**, **metrics**, and **logs**.

## Quick start

```bash
docker compose up -d
```

## Services

| Service        | URL                          | Purpose              |
|----------------|------------------------------|----------------------|
| OTel Collector | `http://localhost:4318`      | OTLP/HTTP receiver   |
| Jaeger         | `http://localhost:16686`     | Trace viewer         |
| Prometheus     | `http://localhost:9090`      | Metrics store        |
| Loki           | `http://localhost:3100`      | Log aggregation      |
| Grafana        | `http://localhost:3000`      | Unified dashboard    |

Grafana login: **admin** / **admin** (or anonymous access is enabled).

## Configuring the Golem OTLP exporter

In your component's `golem.yaml`, add the plugin:

```yaml
components:
  my:component:
    templates: rust
    presets:
      debug:
        plugins:
          - name: golem-otlp-exporter
            version: 1.5.0
            parameters:
              endpoint: "http://localhost:4318"
              signals: "traces,logs,metrics"
```

Then deploy:

```bash
golem deploy --yes
```

## Viewing the data

- **Traces**: Open [Jaeger UI](http://localhost:16686) and search by service name
  (defaults to `{component-uuid}/{worker-name}`).
- **Metrics**: Open [Grafana](http://localhost:3000), go to Explore → Prometheus,
  and query for `golem_*` metrics (e.g. `golem_invocation_count`).
- **Logs**: Open [Grafana](http://localhost:3000), go to Explore → Loki,
  and query `{exporter="OTLP"}` or browse labels.

## Tear down

```bash
docker compose down -v
```

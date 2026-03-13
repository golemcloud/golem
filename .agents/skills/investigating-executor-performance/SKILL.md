---
name: investigating-executor-performance
description: "Investigating worker-executor performance by running executor tests with OTLP tracing enabled and analyzing trace data from Jaeger. Use when diagnosing slow tests, understanding executor call flows, or profiling span durations."
---

# Investigating Worker-Executor Performance

Run worker-executor tests with distributed tracing enabled and analyze the resulting traces in Jaeger to understand performance characteristics, call flows, and bottlenecks.

## Prerequisites

- Docker and Docker Compose installed
- The monitoring stack defined in `integration-tests/monitoring/docker-compose.yml`

## Step 1: Start the Monitoring Stack

From the `integration-tests/monitoring/` directory:

```shell
cd integration-tests/monitoring
docker compose down && docker compose up -d
```

This starts:
- **Jaeger** — UI on `localhost:16686`, OTLP collector on `localhost:4318`
- **Prometheus** — on `localhost:9090`
- **Grafana** — on `localhost:3000` (admin/admin)

## Step 2: Run Tests with OTLP Tracing

Set these environment variables before the cargo make task:

```shell
GOLEM__TRACING__OTLP__ENABLED=true \
GOLEM__TRACING__OTLP__HOST=localhost \
GOLEM__TRACING__OTLP__PORT=4318 \
GOLEM__TRACING__OTLP__SERVICE_NAME=worker-executor-tests \
RUST_LOG=info,h2=warn,hyper=warn \
cargo make worker-executor-tests-group1
```

To run a single test instead of a full group:

```shell
GOLEM__TRACING__OTLP__ENABLED=true \
GOLEM__TRACING__OTLP__HOST=localhost \
GOLEM__TRACING__OTLP__PORT=4318 \
GOLEM__TRACING__OTLP__SERVICE_NAME=worker-executor-tests \
RUST_LOG=info,h2=warn,hyper=warn \
cargo test -p golem-worker-executor --test integration -- <test_name> --report-time --nocapture
```

### How it works

The test `lib.rs` at `golem-worker-executor/tests/lib.rs` initializes tracing via:
```rust
TracingConfig::test_pretty_without_time("worker-executor-tests").with_env_overrides()
```

The `.with_env_overrides()` call uses Figment to merge `GOLEM__*` env vars into the `TracingConfig`, which includes `OtlpConfig` (defined in `golem-common/src/tracing.rs`). Since worker-executor tests run in-process (not spawned as child processes), the OTLP config applies to the single test process directly.

### Suppressing noise

Set `RUST_LOG=info,h2=warn,hyper=warn` to suppress verbose HTTP/2 and Hyper logs that the OTLP exporter generates. Without this, the test output is flooded with transport-level noise.

## Step 3: View Traces in Jaeger

Open `http://localhost:16686` in a browser. Select service `worker-executor-tests` from the dropdown.

### Available test groups

| Task | Tag | Description |
|------|-----|-------------|
| `worker-executor-tests-group1` | group1 | api, blobstore, keyvalue, http, rdbms, agent |
| `worker-executor-tests-group2` | group2 | hot_update, transactions, observability |
| `worker-executor-tests-group3` | group3 | durability, rpc, wasi, scalability, revert |

## Step 4: Analyze Traces via Jaeger API

Jaeger exposes an HTTP API at `localhost:16686`. Use it to programmatically analyze trace data.

### Fetch traces

```shell
# List services (verify the service name appears)
curl -s 'http://localhost:16686/api/services' | python3 -m json.tool

# Fetch traces (limit and lookback are adjustable)
curl -s 'http://localhost:16686/api/traces?service=worker-executor-tests&limit=1000&lookback=1h' \
  -o tmp/traces.json
```

### Analysis patterns

All examples assume traces are saved in `tmp/traces.json`. Span durations in the Jaeger JSON are in **microseconds** (divide by 1000 for milliseconds).

#### Summary statistics

```python
python3 -c "
import json
data = json.load(open('tmp/traces.json'))
traces = data['data']
total_spans = sum(len(t['spans']) for t in traces)
print(f'Traces: {len(traces)}, Total spans: {total_spans}')
"
```

#### Operation name distribution

```python
python3 -c "
import json
from collections import Counter
data = json.load(open('tmp/traces.json'))
ops = Counter()
for t in data['data']:
    for s in t['spans']:
        ops[s['operationName']] += 1
for op, count in ops.most_common(30):
    print(f'{count:6d}  {op}')
"
```

#### Find slowest spans

```python
python3 -c "
import json
data = json.load(open('tmp/traces.json'))
spans = []
for t in data['data']:
    for s in t['spans']:
        spans.append((s['duration']/1000, s['operationName'], s['traceID'][:12]))
spans.sort(reverse=True)
for dur_ms, op, tid in spans[:20]:
    print(f'{dur_ms:10.1f}ms  {op}  trace:{tid}')
"
```

#### Find error spans

```python
python3 -c "
import json
data = json.load(open('tmp/traces.json'))
for t in data['data']:
    for s in t['spans']:
        for tag in s.get('tags', []):
            if tag['key'] == 'otel.status_code' and tag['value'] == 'ERROR':
                dur = s['duration'] / 1000
                print(f'{dur:.1f}ms  {s[\"operationName\"]}  trace:{s[\"traceID\"][:12]}')
"
```

#### Trace size distribution (spans per trace)

```python
python3 -c "
import json
from collections import Counter
data = json.load(open('tmp/traces.json'))
sizes = Counter(len(t['spans']) for t in data['data'])
for size, count in sorted(sizes.items()):
    print(f'{count:4d} traces with {size:4d} spans')
"
```

#### Detect single-span orphan traces

Single-span traces often indicate missing context propagation — the span was created but not linked to a parent trace.

```python
python3 -c "
import json
from collections import Counter
data = json.load(open('tmp/traces.json'))
orphans = Counter()
for t in data['data']:
    if len(t['spans']) == 1:
        orphans[t['spans'][0]['operationName']] += 1
print(f'Total single-span orphan traces: {sum(orphans.values())}')
for op, count in orphans.most_common(15):
    print(f'{count:4d}  {op}')
"
```

#### Identify background noise traces

Long-lived spans from background loops (e.g., "Oplog background transfer", "Scheduler loop") can dominate the trace data. Filter them out for focused analysis:

```python
python3 -c "
import json
data = json.load(open('tmp/traces.json'))
NOISE = {'Oplog background transfer', 'Scheduler loop', 'broadcast loop'}
clean = [t for t in data['data']
         if not any(s['operationName'] in NOISE for s in t['spans'])]
print(f'Total: {len(data[\"data\"])}, After filtering noise: {len(clean)}')
"
```

## Known Caveats

- **Trace context propagation works end-to-end**: The `OtelGrpcLayer` on both client and server sides correctly injects/extracts `traceparent` headers. Test spans, gRPC client spans, and gRPC server handler spans (e.g., `invocation`, `replaying`) all appear in a single connected trace. If you see orphan traces, verify the `GOLEM__TRACING__OTLP__*` env vars are set — without them, the `tracing_opentelemetry` layer is not added to the subscriber, so spans have no OTel context and the propagator injects nothing.
- **Background loop noise**: Long-lived background tasks create traces spanning the entire test duration (~90s). These are not performance issues but can obscure real test traces.
- **Fresh Jaeger**: Always restart Jaeger with `docker compose down && docker compose up -d` before a new investigation to avoid mixing traces from different runs.

## Resetting Between Runs

```shell
cd integration-tests/monitoring
docker compose down && docker compose up -d
```

This clears all stored trace data so the next test run starts fresh.

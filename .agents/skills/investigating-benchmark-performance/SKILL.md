---
name: investigating-benchmark-performance
description: "Investigating benchmark performance by running Golem benchmarks with OTLP tracing enabled and analyzing trace data from Jaeger. Use when diagnosing slow benchmarks, understanding benchmark call flows, or profiling span durations during benchmark runs."
---

# Investigating Benchmark Performance

Run Golem benchmarks with distributed tracing enabled and analyze the resulting traces in Jaeger to understand performance characteristics, call flows, and bottlenecks.

## Prerequisites

- Docker and Docker Compose installed
- The monitoring stack defined in `integration-tests/monitoring/docker-compose.yml`
- The Golem service binaries built with **release** profile: `cargo build --release -p golem-worker-executor ...` (see Step 1.5)
- The benchmark runner binary built with **benchmarks** profile: `cargo build --profile benchmarks -p integration-tests --bin benchmarks` (see Step 1.5)

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

## Step 1.5: Build Binaries

Two separate builds are required:

### Service binaries (release profile)

The benchmark runner in `spawned` mode starts Golem services as child processes. It expects pre-compiled **release** binaries at `target/release/`.

```shell
cargo build --release \
  -p golem-component-compilation-service \
  -p golem-worker-service \
  -p golem-worker-executor \
  -p golem-shard-manager \
  -p golem-registry-service
```

**You must rebuild after every code change** — the benchmark runner uses whatever binaries are on disk. To rebuild only the crate you changed:

```shell
cargo build --release -p <crate-you-changed>
```

### Benchmark runner binary (benchmarks profile)

The benchmark runner itself must be built with the `benchmarks` profile (defined in root `Cargo.toml`, inherits from `release` with `panic = "unwind"`):

```shell
cargo build --profile benchmarks -p integration-tests --bin benchmarks
```

This produces the binary at `target/benchmarks/benchmarks`.

## Step 2: Run Benchmarks with OTLP Tracing

The benchmarks binary is at `integration-tests/src/benchmarks/all.rs` and is built as the `benchmarks` binary from the `integration-tests` crate. It has a built-in `--otlp` flag that configures all spawned Golem services to export traces.

### Running a single benchmark

The CLI takes the benchmark name as a positional argument, followed by the `spawned` subcommand (which tells it to spawn services locally). The `--build-target` defaults to `target/release` which is the correct path for the service binaries.

Run the benchmark using the benchmarks-profile binary:

```shell
./target/benchmarks/benchmarks \
  --otlp \
  benchmark \
  --iterations <N> \
  --cluster-size <S> \
  --size <W> \
  --length <L> \
  <benchmark-name> \
  spawned
```

**Note:** `--size` and `--cluster-size` accept multiple values by repeating the flag (e.g., `--size 1 --size 10`), not comma-separated.

### Available benchmarks

| Name | Description | Recommended quick-run parameters |
|------|-------------|----------------------------------|
| `cold-start-unknown-small` | First-time invocation of a never-instantiated small component | `--size 1,5,10 --length 2 --cluster-size 1` (with and without `--disable-compilation-cache`) |
| `cold-start-unknown-medium` | First-time invocation of a never-instantiated medium component | `--size 1,5,10 --length 5 --cluster-size 1` (with and without `--disable-compilation-cache`) |
| `latency-small` | Cold and hot invocation latency for a small component | `--size 100,500,1000 --length 2 --cluster-size 1` |
| `latency-medium` | Cold and hot invocation latency for a medium component | `--size 100,500,1000 --length 5 --cluster-size 1` |
| `sleep` | Measures sleep/suspend overhead | `--size 10,100,500 --length 10000 --cluster-size 1` |
| `durability-overhead` | Measures the overhead of durable execution | `--size 10,100,1000 --length 5000 --cluster-size 1` |
| `throughput-echo` | Throughput benchmark with echo workload | `--size 1,10,100 --length 1000 --cluster-size 1,5` |
| `throughput-large-input` | Throughput benchmark with large input payloads | `--size 1,10 --length 100,10000,100000 --cluster-size 1,5` |
| `throughput-cpu-intensive` | Throughput benchmark with CPU-intensive workload | `--size 1,10 --length 100 --cluster-size 1,5` |

### Example: single benchmark with tracing

```shell
./target/benchmarks/benchmarks \
  --otlp \
  benchmark \
  --iterations 1 \
  --cluster-size 1 \
  --size 100 \
  --length 2 \
  latency-small \
  spawned
```

### Running a benchmark suite

Benchmark suites are YAML files in `integration-tests/benchmark_suites/`. They define multiple benchmarks with their parameters.

```shell
./target/benchmarks/benchmarks \
  --otlp \
  suite \
  --path integration-tests/benchmark_suites/quick-all.yaml
```

Available suites:
- `quick-all.yaml` — All benchmarks with reduced parameters, suitable for quick local runs
- `ci.yaml` — CI configuration

Suite results can be saved to JSON:

```shell
./target/benchmarks/benchmarks \
  --otlp \
  suite \
  --path integration-tests/benchmark_suites/quick-all.yaml \
  --save-to-json tmp/benchmark-results.json
```

### Additional CLI flags

| Flag | Description |
|------|-------------|
| `--otlp` | Enable OTLP tracing for all spawned services |
| `--json` | Output results as JSON instead of human-readable format |
| `--primary-only` | Only display primary results (no per-worker breakdown) |
| `--quiet` | Reduce log verbosity to warnings only |
| `--verbose` | Increase log verbosity to debug level |

### Benchmark parameters

| Parameter | Meaning |
|-----------|---------|
| `iterations` | Number of times to repeat the benchmark |
| `cluster-size` | Number of worker executors in the test cluster |
| `size` | Workload size (e.g., number of workers) |
| `length` | Workload length (benchmark-specific, e.g., payload size, iteration count) |
| `disable-compilation-cache` | Disable compilation caching to measure cold compilation |

## Step 3: View Traces in Jaeger

Open `http://localhost:16686` in a browser. The benchmarks service name depends on the spawned services — look for service names like `worker-executor`, `worker-service`, `component-compilation-service`, etc.

## Step 4: Analyze Traces via Jaeger API

Jaeger exposes an HTTP API at `localhost:16686`. Use it to programmatically analyze trace data.

### Fetch traces

```shell
# List services (to find the correct service names)
curl -s 'http://localhost:16686/api/services' | python3 -m json.tool

# Fetch traces for a specific service
curl -s 'http://localhost:16686/api/traces?service=worker-executor&limit=1000&lookback=1h' \
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

Long-lived spans from background loops can dominate the trace data. Filter them out for focused analysis:

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

- **Multiple services**: Unlike worker-executor tests (which run in-process), benchmarks spawn separate Golem services. The `--otlp` flag configures all spawned services to export traces. Look for multiple service names in Jaeger.
- **Trace context propagation**: gRPC calls between services propagate trace context via `traceparent` headers. If you see disconnected traces, verify the monitoring stack is running before starting the benchmark.
- **Span queue size (`OTEL_BSP_MAX_QUEUE_SIZE`)**: The `BatchSpanProcessor` has a default queue size of 2048 spans. Under high-throughput benchmarks this queue can overflow, causing spans to be silently dropped (logged as `BatchSpanProcessor dropped a Span due to queue full`). The test framework automatically sets `OTEL_BSP_MAX_QUEUE_SIZE=65536` for all spawned services when `--otlp` is enabled. For the benchmark runner process itself, set it manually if needed: `OTEL_BSP_MAX_QUEUE_SIZE=65536 ./target/benchmarks/benchmarks --otlp ...`.
- **Background loop noise**: Long-lived background tasks create traces spanning the entire benchmark duration. These are not performance issues but can obscure real benchmark traces.
- **Fresh Jaeger**: Always restart Jaeger with `docker compose down && docker compose up -d` before a new investigation to avoid mixing traces from different runs.
- **Build time**: Service binaries must be built with `--release`. The benchmark runner binary must be built with `--profile benchmarks`. After any code change, rebuild the affected service binary with `cargo build --release -p <crate>` before re-running the benchmark.

## Resetting Between Runs

```shell
cd integration-tests/monitoring
docker compose down && docker compose up -d
```

This clears all stored trace data so the next benchmark run starts fresh.

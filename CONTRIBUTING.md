## Running integration tests

Install [cargo make](https://github.com/sagiegurari/cargo-make)

```shell
cargo install --force cargo-make
```

runs all unit tests, worker executor tests and integration tests

```shell
cargo make test
```

runs unit tests only

```shell
cargo make unit-tests
```

runs worker executor tests only

```shell
cargo make integration-tests
```

runs CLI tests only

```shell
cargo make cli-tests
```

runs sharding integration tests only

```shell
cargo make sharding-tests
```

## Running Benchmarks

1. Raise PR
2. Reviewer or author of PR can run benchmarks by typing in a PR comment as follows
```shell
    /run-benchmark
```

3. For all new benchmark types (meaning, those for which there is no baseline to compare), it should generate a report as below, as a PR comment

## Benchmark Report
| Benchmark Type | Cluster Size | Size | Length | Avg Time |
|---------------|--------------|------|--------|----------|
| benchmark_cold_start_large.json | 3 | 10 | 100 | 201.255108ms |
| benchmark_cold_start_large_no_compilation.json | 3 | 10 | 100 | 123.000794122s |
| benchmark_cold_start_medium.json | 3 | 10 | 100 | 121.566283ms |
| benchmark_cold_start_medium_no_compilation.json | 3 | 10 | 100 | 178.508111048s |
| benchmark_cold_start_small.json | 3 | 10 | 100 | 75.379351ms |
| benchmark_cold_start_small_no_compilation.json | 3 | 10 | 100 | 423.142651ms |
| benchmark_durability_overhead.json | 3 | 10 | 100 | 57.51445ms |
| benchmark_latency_large.json | 3 | 10 | 100 | 61.586289ms |
| benchmark_latency_medium.json | 3 | 10 | 100 | 60.646373ms |
| benchmark_latency_small.json | 3 | 10 | 100 | 54.76123ms |
| benchmark_suspend_worker.json | 3 | 10 | 100 | 10.03030193s |

RunID: 9435476881

4. The underlying data used to created the above report will be automatically pushed back to the PR branch
5. If there exists a baseline to compare for the benchmark type, then a comparison report will be generated for those benchmarks
6. If there is no need to compare with any baseline, regardless of a baseline exist or not, then simply run

```bash

/run-benchmark-refresh

```
7. Refresh message can be useful in the event of comparison failures (Example: A failure due to schema mismatch especially when a developer refactor the benchmark code itself)

## Starting all services locally

There is a simple `cargo make run` task that starts all the debug executables of the services locally, using the default configuration. The prerequisites are:

- `nginx` installed (on OSX can be installed with `brew install nginx`)
- `redis` installed (on OSX can be installed with `brew install redis`)
- `lnav` installed (on OSX can be installed with `brew install lnav`)

The `cargo make run` command will start all the services and show a unified view of the logs using `lnav`. Quitting `lnav` kills the spawned processes.

## Local Testing using Docker containers

We recommend using linux VMs (if you are on MAC) to build the docker. 

### Example: 

Use `multipass` or similar platforms, with a reasonable disk space (since it's the easiest to get a ubuntu)
Running this in a MAC,can result in various issues that comes with cross compilation. 

```bash
cd golem
export PLATFORM_OVERRIDE = "linux/arm64"
cargo make build-release-override-linux-arm64
docker compose -f docker-compose-sqlite.yaml up --build
```

To start the service without a rebuild

```bash

docker compose -f docker-compose-sqlite.yaml up

```

To compose down,

```bash

docker compose -f docker-compose-sqlite.yaml down -v

```

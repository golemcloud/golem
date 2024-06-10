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

3. If there are no other benchmarks to compare the following report should generate as a PR comment itself

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

4. The underlying data used to created the above create will be automatically pushed back to the PR branch itself
5. If there exists a baseline to compare for the benchmark type, then a comparison report will be generated for these benchmarks, and rest of them will be reported as above
6. If there is no need to compare with any baseline, regardless of a baseline exist or not, then simply run

```bash

/run-benchmark-refresh

```
7. Refresh message can be useful in the event of comparison failures due to schema mismatches (especially when a developer refactor the benchmark code itself)

## Starting all services locally

There is a simple `cargo make run` task that starts all the debug executables of the services locally, using the default configuration. The prerequisites are:

- `nginx` installed (on OSX can be installed with `brew install nginx`)
- `redis` installed (on OSX can be installed with `brew install redis`)
- `lnav` installed (on OSX can be installed with `brew install lnav`)

The `cargo make run` command will start all the services and show a unified view of the logs using `lnav`. Quitting `lnav` kills the spawned processes.

## Local Testing using Docker containers

To spin up services using the latest code

```bash
# Clone golem-services
cd golem-services

# Find more info below if you are having issues running this command(example: Running from MAC may fail)
# Target has to be x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu-gcc
cargo build --release --target x86_64-unknown-linux-gnu

docker compose -f docker-compose-sqlite.yaml up --build
```

To start the service without a rebuild

```bash

docker compose -f docker-compose-sqlite.yaml up

```

To compose down,

```bash

docker compose -f docker-compose-sqlite.yaml down

```

To compose down including persistence volume

```bash

docker compose -f docker-compose-sqlite.yaml down -v

```

Note that, if you are using MAC, the persistene volumes may be present in the Linux VM. You can inspect this using the following command:

```bash

docker run -it --rm --privileged --pid=host alpine:latest nsenter -t 1 -m -u -n -i sh

# As an example: cd /var/lib/docker/volumes/golem-services_redis_data/_data
/var/lib/docker/volumes/golem-services_redis_data/_data # ls -lrt
total 4
-rw-------    1 999      ping          3519 Jan 19 02:32 dump.rdb
/var/lib/docker/volumes/golem-services_redis_data/_data #

```

If you have issues running the above cargo build command, then read on:

Make sure to do `docker-compose pull` next time to make sure you are pulling the latest images than the cached ones

### Cargo Build

### MAC

If you are running ` cargo build --target ARCH-unknown-linux-gnu` (cross compiling to Linux) from MAC, you may encounter
some missing dependencies. If interested, refer, https://github.com/messense/homebrew-macos-cross-toolchains

### Intel MAC

Typically, the following should allow you to run it successfully.

```bash
brew tap messense/macos-cross-toolchains
brew install messense/macos-cross-toolchains/x86_64-unknown-linux-gnu
# If openssl is not in system
# brew install openssl
export OPENSSL_DIR=$(brew --prefix openssl)
export CC_X86_64_UNKNOWN_LINUX_GNU=x86_64-unknown-linux-gnu-gcc
export CXX_X86_64_UNKNOWN_LINUX_GNU=x86_64-unknown-linux-gnu-g++
export AR_X86_64_UNKNOWN_LINUX_GNU=x86_64-unknown-linux-gnu-ar
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-unknown-linux-gnu-gcc
```

From the root of the project

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu --package golem-shard-manager
cargo build --release --target x86_64-unknown-linux-gnu --package golem-component-service
cargo build --release --target x86_64-unknown-linux-gnu --package golem-worker-service
cargo build --release --target x86_64-unknown-linux-gnu --package golem-component-compilation-service
cargo build --release --target x86_64-unknown-linux-gnu --package golem-worker-executor
```

### ARM MAC

Typically, the following should allow you to run it successfully.

```bash
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
# If openssl is not in system
# brew install openssl
export OPENSSL_DIR=$(brew --prefix openssl)
export CC_AARCH64_UNKNOWN_LINUX_GNU=aarch64-unknown-linux-gnu-gcc
export CXX_AARCH64_UNKNOWN_LINUX_GNU=aarch64-unknown-linux-gnu-g++
export AR_AARCH64_UNKNOWN_LINUX_GNU=aarch64-unknown-linux-gnu-ar
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-unknown-linux-gnu-gcc
```

From the root of the project

```bash
rustup target add aarch64-unknown-linux-gnu-gcc
cargo build --release --target aarch64-unknown-linux-gnu --package golem-shard-manager
cargo build --release --target aarch64-unknown-linux-gnu --package golem-component-service
cargo build --release --target aarch64-unknown-linux-gnu --package golem-worker-service
cargo build --release --target aarch64-unknown-linux-gnu --package golem-component-compilation-service
cargo build --release --target aarch64-unknown-linux-gnu --package golem-worker-executor
```

### LINUX

From the root of the project

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu --package golem-shard-manager
cargo build --release --target x86_64-unknown-linux-gnu --package golem-component-service
cargo build --release --target x86_64-unknown-linux-gnu --package golem-worker-service
cargo build --release --target x86_64-unknown-linux-gnu --package golem-component-compilation-service
cargo build --release --target x86_64-unknown-linux-gnu --package golem-worker-executor
```


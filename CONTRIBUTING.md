# Golem development


## Prerequisites

To work on **Golem** you need to install (via package installers or manually) the following tools:

- [rustup](https://rustup.rs/)
- [Protobuf](https://github.com/protocolbuffers/protobuf#protobuf-compiler-installation)
  - The [prost crate](https://crates.io/crates/prost) requires `protoc`
  - Requires **version 28** or later
- For integration tests  
  - [redis](https://redis.io/downloads/)
  - [docker](https://www.docker.com)
- To be able to run all services with `cargo-make run` with a merged log view:
  - [lnav](https://lnav.org)
  - [nginx](https://nginx.org)

### OSX
```sh
brew install rustup protobuf redis docker lnav nginx
```

## Rust Installation
```sh
# latest **stable** rust compiler
rustup update stable
rustup default stable
rustup target add wasm32-wasip1
cargo install --force cargo-make
```

Everything else is managed by [cargo-make](https://github.com/sagiegurari/cargo-make).

## Development workflow

### Building
To compile everything use

```shell
cargo make build
```
It is recommended to do a full build before starting working on Golem and opening it with an IDE. During development it is usually enough to recompile only the crate you are working on, for example:

```shell
cargo build -p golem-worker-service-base
```

#### If cargo runs out of memory
Depending on the number of CPU cores and available memory, building everything can use a lot of memory. If cargo runs out of memory or just uses too much, you can limit the number of parallel jobs by providing a cargo `config.toml` file, for example:

```toml
[build]
jobs = 4
```

in `~/.cargo/config.toml`. For other options and possibilities check [the cargo documentation](https://doc.rust-lang.org/cargo/reference/config.html).

### Running tests

Tests are using the [test-r library](https://test-r.vigoo.dev).

During development you can run the involved tests in the usual ways: from the IDE, or using `cargo test` command selecting a specific crate and test module, for example:

```shell
cargo test -p golem-worker-executor api::promise -- --report-time
```

#### Running all unit tests
To run all unit tests use

```shell
cargo make unit-tests
```

#### Running all worker executor tests
The **worker executor tests** are testing the Golem Worker Executor standalone without any of the other services. These tests require `redis`. To run all of them use

```shell
cargo make worker-executor-tests
```

As there are many of these tests, they are organized into **groups** that are executed in parallel on CI. You can run only a specific group with cargo make, for example:

```shell
cargo make worker-executor-tests-group1
```

#### Running all integration tests
The **integration tests** are starting up several Golem services and testing them together. These tests also require `docker` and `redis` to be available.

To run all integration tests use

```shell
cargo make integration-tests
```

#### Running all the CLI tests
The **CLI tests** are similar to the integration tests but interact with the running services only through the Golem CLI application.

To run all CLI tests use

```shell
cargo make cli-tests
```

#### Using a debugger
When using a debugger with the tests, make sure to pass the `--nocapture` option to the test runner, otherwise the debugger will not be usable (when capturing is on, the test framework spawns child processes to run the actual tests).

### Updating the REST API
Golem **generates OpenAPI specs** from the Rust code (using the [poem-openapi crate](https://crates.io/crates/poem-openapi), and the generated OpenAPI yaml file is also stored in the repository and a Rust Client crate is generated from it, used by the CLI app and also published into crates.io.

When changing anything that affects the user facing REST API, this YAML needs to be explicitly regenerated. If this does not happen, the CI process will fail and ask for doing it.

To regenerate the OpenAPI spec use

```shell
cargo make generate-openapi
```

### Updating the config files
Service config files are also generated from code similarly to OpenAPI specs. When changing any of the service configuration data types, they have to be regeneraetd otherwise the CI process fails and asks for doing it.

To regenerate these files, use

```shell
cargo make generate-configs
```

### Preparing the pull request
Golem CI checks the pull requests with `rustfmt` and `cargo clippy`. To make sure these checks pass, before opening a pull request run

```shell
cargo make fix
```

and fix any possible errors and warnings reported by it.

## Running Golem locally

There are two ways now to run Golem locally:

### Using cargo make run

By running

```shell
cargo make run
```

all the services are going to be built and started as individual native processes, configured to communicate with each other. Beside that, an `nginx` process is going to be started to unify the various processes' HTTP APIs, and `lnav` is started to merge their logs.

### Using the Single Golem Executable

Golem now contains a **single executable** crate, called `golem`, that links all the services into a single binary. This binary is built using a regular `cargo make build` run and can be executed with

```shell
cargo run -p golem
```

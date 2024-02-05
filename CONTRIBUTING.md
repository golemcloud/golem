## Submodules

To initialise git submodules you can use this command:
```shell
git submodule update --init --recursive
```

## Running integration tests

Integration tests are stored in `golem-cli/tests` directory.

You can run all tests with
```shell
./scripts/it.sh
```

To run individual tests you should first build all executables with `./scripts/build-all.sh` and then run tests in `golem-cli` directories:
```shell
cargo test worker_new_instance
```

With `QUIET=true` you can hide services output:
```shell
QUIET=true cargo test
```

### Running integration tests without docker

Docker is used to run `Postgres` for `golem-services`. You can configure tests to use `Sqlite` DB with `GOLEM_TEST_DB=Sqlite`.
```shell
GOLEM_TEST_DB=Sqlite RUST_LOG=info cargo test
```

## Local Testing

To spin up services using the latest code

```bash
# Clone golem-services
cd golem-services

# init & update git submodules
git submodule update --init --recursive

# Find more info below if you are having issues running this command(example: Running from MAC may fail)
# Target has to be x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu-gcc
cargo build --release --target x86_64-unknown-linux-gnu

docker-compose -f docker-compose-sqlite.yaml up --build
```
To start the service without a rebuild

```bash

docker-compose -f docker-compose-sqlite.yaml up

```

To compose down,

```bash

docker-compose -f docker-compose-sqlite.yaml down

```

To compose down including persistence volume

```bash

docker-compose -f docker-compose-sqlite.yaml down -v

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
brew install x86_64-unknown-linux-gnu
export CC_X86_64_UNKNOWN_LINUX_GNU=x86_64-unknown-linux-gnu-gcc
export CXX_X86_64_UNKNOWN_LINUX_GNU=x86_64-unknown-linux-gnu-g++
export AR_X86_64_UNKNOWN_LINUX_GNU=x86_64-unknown-linux-gnu-ar
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-unknown-linux-gnu-gcc
```

From the root of the project

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --target x86_64-unknown-linux-gnu
```

### ARM MAC

Typically, the following should allow you to run it successfully.

```bash
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu
export CC_AARCH64_UNKNOWN_LINUX_GNU=aarch64-unknown-linux-gnu-gcc
export CXX_AARCH64_UNKNOWN_LINUX_GNU=aarch64-unknown-linux-gnu-g++
export AR_AARCH64_UNKNOWN_LINUX_GNU=aarch64-unknown-linux-gnu-ar
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-unknown-linux-gnu-gcc
```

From the root of the project

```bash
rustup target add aarch64-unknown-linux-gnu-gcc
cargo build --target aarch64-unknown-linux-gnu-gcc
```

### LINUX

From the root of the project

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --target x86_64-unknown-linux-gnu
```

### WINDOWS
TBD

We will be trying cargo chef and offload cargo build to docker context without impacting the build time.

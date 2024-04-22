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

## Local Testing

To spin up services using the latest code

```bash
# Clone golem-services
cd golem-services

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
cargo build --release --target aarch64-unknown-linux-gnu --package golem-shard-manager
cargo build --release --target aarch64-unknown-linux-gnu --package golem-component-service
cargo build --release --target aarch64-unknown-linux-gnu --package golem-worker-service
cargo build --release --target aarch64-unknown-linux-gnu --package golem-component-compilation-service
cargo build --release --target aarch64-unknown-linux-gnu --package golem-worker-executor
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
cargo build --release --target aarch64-unknown-linux-gnu --package golem-shard-manager
cargo build --release --target aarch64-unknown-linux-gnu --package golem-component-service
cargo build --release --target aarch64-unknown-linux-gnu --package golem-worker-service
cargo build --release --target aarch64-unknown-linux-gnu --package golem-component-compilation-service
cargo build --release --target aarch64-unknown-linux-gnu --package golem-worker-executor
```

## Integration with existing API Gateways

Please refer to [api-gateway-examples](api-gateway-examples) for more information.
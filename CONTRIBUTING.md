## Local Testing

To spin up services using the latest code

```bash
# Clone golem-services
cd golem-services
# Find more info below if you are having issues running this command(example: Running from MAC may fail)
# Target has to be x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu-gcc
cargo build --release --target x86_64-unknown-linux-gnu

docker-compose up --build
```
To start the service without a rebuild

```bash

docker-compose up

```

To run the services in background

```bash
docker-compose up -d
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
# golem-services

This repository contains the open source parts of Golem Cloud - a set of services enable you to run WebAssembly components in a distributed cloud environment.

See [Golem Cloud](https://golem.cloud) for more information.

## Local Testing

To spin up services

```bash
docker-compose up
```

Note that docker-compose up alone will not rebuild the changes.
So run cargo build first, and in this case you need to specify
the target x86_64-unknown-linux-gnu

```bash
cargo build --target x86_64-unknown-linux-gnu
```

The docker-compose internally assumes your image is built for Linux.
Meaning, we are compiling golem services to target Linux container.

Once the cargo build is successful, simply run

```bash
# from the root of the project
# --build will rebuild the images based on the cargo targets in the previous step
docker-compose up --build
```

If you have issues running the above cargo build command, then read on:

### Cargo Build 

### MAC
If you are running ` cargo build --target x86_64-unknown-linux-gnu` (cross compiling to Linux) from MAC, you may encounter
some missing dependencies. If interested, refer, https://github.com/messense/homebrew-macos-cross-toolchains

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

### LINUX

From the root of the project

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --target x86_64-unknown-linux-gnu
```

### WINDOWS
TBD


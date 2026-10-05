# The environment bash.wasm is built in (../build-bash-wasm.sh): the Rust toolchain
# ../rust-toolchain.toml names, on Linux x86-64, and wasm-tools, each pinned by digest.
#
# The platform is part of the pin. Cargo mixes the host into the hash that names each proc-macro
# and build script, and from there into the hash of every crate that uses one, so the same
# sources give different bytes when built on a different host.
FROM rust:1.98.0-slim-bookworm@sha256:1469a27c125cb5a3aebfa4f4e4665d935b02fb72cc093b2c974b3d740e43f157

RUN rustup target add wasm32-wasip2 && rustup component add rustfmt clippy

ADD --checksum=sha256:08d523676ec71d9afbae05aa4255041ce91bf2d325d87b7e722d190d558be689 \
  https://github.com/bytecodealliance/wasm-tools/releases/download/v1.251.0/wasm-tools-1.251.0-x86_64-linux.tar.gz \
  /tmp/wasm-tools.tar.gz
RUN tar -xzf /tmp/wasm-tools.tar.gz -C /tmp \
  && mv /tmp/wasm-tools-1.251.0-x86_64-linux/wasm-tools /usr/local/bin/wasm-tools \
  && rm -rf /tmp/wasm-tools.tar.gz /tmp/wasm-tools-1.251.0-x86_64-linux

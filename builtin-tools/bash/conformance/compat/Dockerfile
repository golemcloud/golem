# Runs Brush's compatibility suite against the bash tool's shell: Linux, GNU tools, bash 5 as the
# oracle, Wasmtime for the shell under test. Used by run-in-docker.sh and the slow CI job.
FROM rust:1.98-bookworm
ARG TARGETARCH
ARG WASMTIME=46.0.1
RUN arch=$([ "$TARGETARCH" = "arm64" ] && echo aarch64 || echo x86_64) \
 && curl -fsSL "https://github.com/bytecodealliance/wasmtime/releases/download/v${WASMTIME}/wasmtime-v${WASMTIME}-${arch}-linux.tar.xz" \
  | tar -xJ -C /opt \
 && ln -s /opt/wasmtime-v${WASMTIME}-${arch}-linux/wasmtime /usr/local/bin/wasmtime \
 && useradd -m -u 12345 runner
ENV LC_ALL=C.UTF-8

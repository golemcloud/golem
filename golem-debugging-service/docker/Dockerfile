FROM ubuntu:latest as base

ARG TARGETARCH

FROM base AS platform-amd64

ENV RUST_TARGET=x86_64-unknown-linux-gnu

FROM base AS platform-arm64

ENV RUST_TARGET=aarch64-unknown-linux-gnu

FROM platform-${TARGETARCH} AS final

WORKDIR /app
COPY /target/$RUST_TARGET/release/golem-debugging-service ./
COPY /golem-debugging-service/config/debug-worker-executor.toml ./config/debug-worker-executor.toml

RUN apt-get update && apt-get install -y pkg-config libssl-dev ca-certificates tini
RUN update-ca-certificates

EXPOSE 8080
EXPOSE 9090

ENTRYPOINT ["/usr/bin/tini", "--", "./golem-debugging-service"]

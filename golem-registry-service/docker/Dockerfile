FROM ubuntu:latest AS base

ARG TARGETARCH

FROM base AS platform-amd64

ENV RUST_TARGET=x86_64-unknown-linux-gnu

FROM base AS platform-arm64

ENV RUST_TARGET=aarch64-unknown-linux-gnu

FROM platform-${TARGETARCH} AS final

WORKDIR /app
COPY /target/$RUST_TARGET/release/golem-registry-service ./
COPY /golem-registry-service/config/registry-service.toml ./config/registry-service.toml

RUN apt-get update && apt-get install -y pkg-config libssl-dev ca-certificates tini
RUN update-ca-certificates

EXPOSE 8081
EXPOSE 9091

ENTRYPOINT ["/usr/bin/tini", "--", "./golem-registry-service"]

FROM debian:bookworm-slim as base

ARG TARGETARCH

FROM base AS platform-amd64

ENV RUST_TARGET=x86_64-unknown-linux-gnu

FROM base AS platform-arm64

ENV RUST_TARGET=aarch64-unknown-linux-gnu

FROM platform-${TARGETARCH} AS final

WORKDIR /app
COPY /target/$RUST_TARGET/release/golem-shard-manager ./
COPY /golem-shard-manager/config/shard-manager.toml ./config/shard-manager.toml

RUN apt-get update && apt-get install -y pkg-config libssl-dev ca-certificates
RUN update-ca-certificates

EXPOSE 8081
EXPOSE 9001

ENTRYPOINT ["./golem-shard-manager"]


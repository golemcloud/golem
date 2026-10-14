FROM ubuntu:latest as base

ARG TARGETARCH

FROM base AS platform-amd64

ENV RUST_TARGET=x86_64-unknown-linux-gnu
LABEL cloud.golem.cpu.target="x86-64-v3"
LABEL cloud.golem.cpu.features=""

FROM base AS platform-arm64

ENV RUST_TARGET=aarch64-unknown-linux-gnu
LABEL cloud.golem.cpu.target="aarch64"
LABEL cloud.golem.cpu.features="neon,crc,lse,aes,sha2,dotprod"

FROM platform-${TARGETARCH} AS final

WORKDIR /app
COPY /target/$RUST_TARGET/release/golem-shard-manager ./
COPY /golem-shard-manager/config/shard-manager.toml ./config/shard-manager.toml

RUN apt-get update && apt-get install -y pkg-config libssl-dev ca-certificates tini
RUN update-ca-certificates

# Runs the service as uid and gid 65534; see the script.
RUN --mount=type=bind,source=docker/prepare-service-image.sh,target=/tmp/prepare-service-image.sh \
    sh /tmp/prepare-service-image.sh

ENV HOME=/home/golem
USER 65534:65534

EXPOSE 8081
EXPOSE 9001

ENTRYPOINT ["/usr/bin/tini", "--", "./golem-shard-manager"]

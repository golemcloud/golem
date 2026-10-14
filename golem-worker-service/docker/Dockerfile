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
COPY /target/$RUST_TARGET/release/golem-worker-service ./
COPY /golem-worker-service/config/worker-service.toml ./config/worker-service.toml

RUN apt-get update && apt-get install -y pkg-config libssl-dev ca-certificates tini
RUN update-ca-certificates

# The service runs as uid and gid 65534. The image gives that id to its own user and group golem,
# in place of the base image's user and group with that id (nobody and nogroup), so that the user
# has a home for caches such as the Wasmtime cache. The group keeps its other members. The default
# configuration keeps local data below /data, which the user owns.
RUN if user=$(getent passwd 65534); then userdel "${user%%:*}"; fi \
    && if group=$(getent group 65534); then groupmod --new-name golem "${group%%:*}"; \
       else groupadd --gid 65534 golem; fi \
    && useradd --uid 65534 --gid 65534 --home-dir /home/golem --create-home \
        --shell /usr/sbin/nologin golem \
    && install --directory --owner 65534 --group 65534 --mode 0750 /data /data/blob_storage

# No program in the image gains privileges when it runs: the image clears every setuid and setgid
# bit, so a process in the container cannot become root through a program of the base image.
RUN find / -xdev -perm /6000 -type f -exec chmod a-s {} +

ENV HOME=/home/golem
USER 65534:65534

EXPOSE 9005

ENTRYPOINT ["/usr/bin/tini", "--", "./golem-worker-service"]

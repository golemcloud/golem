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

# Gives the executor binary the file capability cap_sys_admin+p: permitted, not effective. The
# kernel grants it only when the container's bounding set holds SYS_ADMIN and no_new_privs is off.
# Only the XFS quota helper uses it: for ManagedXfs storage the executor starts the helper from
# this binary, and then drops the capability itself. With the effective bit, the kernel would
# refuse to run the binary in a container without SYS_ADMIN. A separate stage keeps the binary in
# one layer of the final image: setcap in the final stage would copy it again.
FROM platform-${TARGETARCH} AS capability

RUN apt-get update && apt-get install -y libcap2-bin
COPY /target/$RUST_TARGET/release/worker-executor /worker-executor
RUN setcap cap_sys_admin+p /worker-executor

FROM platform-${TARGETARCH} AS final

# The image has no time zone database. jiff reads this POSIX rule without one, so the snapshot
# store writes no time zone warning for each timestamp.
ENV TZ=UTC0

WORKDIR /app
COPY --from=capability /worker-executor ./
COPY /golem-worker-executor/config/worker-executor.toml ./config/worker-executor.toml

RUN apt-get update && apt-get install -y pkg-config libssl-dev ca-certificates tini
RUN update-ca-certificates

# Runs the service as uid and gid 65534; see the script.
RUN --mount=type=bind,source=docker/prepare-service-image.sh,target=/tmp/prepare-service-image.sh \
    sh /tmp/prepare-service-image.sh

ENV HOME=/home/golem
USER 65534:65534

EXPOSE 9000

ENTRYPOINT ["/usr/bin/tini", "--", "./worker-executor"]

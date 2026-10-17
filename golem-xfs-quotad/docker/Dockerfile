FROM ubuntu:latest AS base

ARG TARGETARCH

FROM base AS platform-amd64

ENV RUST_TARGET=x86_64-unknown-linux-gnu
LABEL cloud.golem.cpu.target="x86-64-v3"
LABEL cloud.golem.cpu.features=""

FROM base AS platform-arm64

ENV RUST_TARGET=aarch64-unknown-linux-gnu
LABEL cloud.golem.cpu.target="aarch64"
LABEL cloud.golem.cpu.features="neon,crc,lse,aes,sha2,dotprod"

# Gives the binary the file capability cap_sys_admin+ep, so that xfs-quotad holds CAP_SYS_ADMIN
# without running as root. Linux grants it only when the container adds SYS_ADMIN and allows
# privilege escalation; in any other container the binary does not start. This stage keeps
# libcap2-bin out of the final image.
FROM platform-${TARGETARCH} AS capability

RUN apt-get update && apt-get install -y libcap2-bin
COPY /target/$RUST_TARGET/release/golem-xfs-quotad /golem-xfs-quotad
RUN setcap cap_sys_admin+ep /golem-xfs-quotad

FROM platform-${TARGETARCH} AS final

WORKDIR /app
COPY --from=capability /golem-xfs-quotad ./

RUN apt-get update && apt-get install -y tini

# Runs xfs-quotad as uid and gid 65534; see the script. /run/golem-quota is the directory of the
# socket in the documented deployments. A new Docker volume that is mounted there takes the owner
# of this directory, so xfs-quotad can create its socket in it.
RUN --mount=type=bind,source=docker/prepare-service-image.sh,target=/tmp/prepare-service-image.sh \
    sh /tmp/prepare-service-image.sh /run/golem-quota

USER 65534:65534

ENTRYPOINT ["/usr/bin/tini", "--", "./golem-xfs-quotad"]

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

FROM platform-${TARGETARCH} AS final

ARG TARGETARCH
ARG LITESTREAM_VERSION=0.5.17
ARG S5CMD_VERSION=2.3.0

# The image has no time zone database. jiff reads this POSIX rule without one, so the snapshot
# store writes no time zone warning for each timestamp.
ENV TZ=UTC0

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl tini \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Litestream replicates the SQLite databases of the data directory, and s5cmd lists the databases
# of an S3 replica, so the entrypoint can restore the ones that are missing.
RUN set -eu; \
    case "${TARGETARCH}" in \
      amd64) \
        litestream_arch=x86_64; \
        litestream_sha256=cfb371176d164437ae869f8351cfde49bd1804ae71c61923f75c9cba9c9c006d; \
        s5cmd_arch=64bit; \
        s5cmd_sha256=de0fdbfa3aceae55e069ba81a0fc17b2026567637603734a387b2fca06c299b4 ;; \
      arm64) \
        litestream_arch=arm64; \
        litestream_sha256=f8ca4a050095c1efbda2c4365172e61bf9d955ea0d9ac42f448b52e51819baa5; \
        s5cmd_arch=arm64; \
        s5cmd_sha256=1439f0d00ecedcd2a2f1f2c6749bbb0152b2257bf5086f29646ec8ae38798e24 ;; \
      *) echo "Unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/litestream.tar.gz \
      "https://github.com/benbjohnson/litestream/releases/download/v${LITESTREAM_VERSION}/litestream-${LITESTREAM_VERSION}-linux-${litestream_arch}.tar.gz"; \
    echo "${litestream_sha256}  /tmp/litestream.tar.gz" | sha256sum -c -; \
    tar --no-same-owner -xzf /tmp/litestream.tar.gz -C /usr/local/bin litestream; \
    curl -fsSL -o /tmp/s5cmd.tar.gz \
      "https://github.com/peak/s5cmd/releases/download/v${S5CMD_VERSION}/s5cmd_${S5CMD_VERSION}_Linux-${s5cmd_arch}.tar.gz"; \
    echo "${s5cmd_sha256}  /tmp/s5cmd.tar.gz" | sha256sum -c -; \
    tar --no-same-owner -xzf /tmp/s5cmd.tar.gz -C /usr/local/bin s5cmd; \
    rm /tmp/litestream.tar.gz /tmp/s5cmd.tar.gz

COPY /target/$RUST_TARGET/release/golem /usr/local/bin/golem
COPY /cli/golem/docker/golem-entrypoint.sh /usr/local/bin/golem-entrypoint
COPY /cli/golem/docker/litestream.yml /etc/golem/litestream.yml

ENV GOLEM__DATA_DIR=/var/lib/golem
ENV GOLEM__ROUTER__ADDRESS="::"
ENV GOLEM__ROUTER__PORT=9881

VOLUME /var/lib/golem
EXPOSE 9881

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/golem-entrypoint"]
CMD ["server", "run"]

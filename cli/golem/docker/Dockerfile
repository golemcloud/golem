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

# The image has no time zone database. jiff reads this POSIX rule without one, so the snapshot
# store writes no time zone warning for each timestamp.
ENV TZ=UTC0

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl tini \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Litestream replicates the SQLite databases of the data directory when GOLEM_LITESTREAM=true.
RUN set -eu; \
    case "${TARGETARCH}" in \
      amd64) \
        litestream_arch=x86_64; \
        litestream_sha256=cfb371176d164437ae869f8351cfde49bd1804ae71c61923f75c9cba9c9c006d ;; \
      arm64) \
        litestream_arch=arm64; \
        litestream_sha256=f8ca4a050095c1efbda2c4365172e61bf9d955ea0d9ac42f448b52e51819baa5 ;; \
      *) echo "Unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/litestream.tar.gz \
      "https://github.com/benbjohnson/litestream/releases/download/v${LITESTREAM_VERSION}/litestream-${LITESTREAM_VERSION}-linux-${litestream_arch}.tar.gz"; \
    echo "${litestream_sha256}  /tmp/litestream.tar.gz" | sha256sum -c -; \
    tar --no-same-owner -xzf /tmp/litestream.tar.gz -C /usr/local/bin litestream; \
    rm /tmp/litestream.tar.gz

COPY /target/$RUST_TARGET/release/golem /usr/local/bin/golem
COPY /cli/golem/docker/golem-entrypoint.sh /usr/local/bin/golem-entrypoint
COPY /cli/golem/docker/litestream.yml /etc/golem/litestream.yml

# Runs the server as uid and gid 65534; see the script. A new volume at /var/lib/golem gets the
# owner of the directory in the image.
RUN --mount=type=bind,source=docker/prepare-service-image.sh,target=/tmp/prepare-service-image.sh \
    sh /tmp/prepare-service-image.sh /var/lib/golem

ENV HOME=/home/golem
ENV GOLEM__DATA_DIR=/var/lib/golem
ENV GOLEM__ROUTER__ADDRESS="::"
ENV GOLEM__ROUTER__PORT=9881

USER 65534:65534
VOLUME /var/lib/golem
EXPOSE 9881

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/golem-entrypoint"]
CMD ["server", "run"]

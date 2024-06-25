FROM debian:bookworm-slim as base

ARG TARGETARCH

FROM base AS platform-amd64

ENV RUST_TARGET=x86_64-unknown-linux-gnu

FROM base AS platform-arm64

ENV RUST_TARGET=aarch64-unknown-linux-gnu

FROM platform-${TARGETARCH} AS final

WORKDIR /app
COPY /target/$RUST_TARGET/release/golem-worker-service ./
COPY /golem-worker-service/config/worker-service.toml ./config/worker-service.toml
COPY /golem-worker-service/db ./db

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

EXPOSE 9005

ENTRYPOINT ["./golem-worker-service"]
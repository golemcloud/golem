FROM debian:bookworm-slim as base

ARG TARGETARCH

FROM base AS platform-amd64

ENV RUST_TARGET=x86_64-unknown-linux-gnu

FROM base AS platform-arm64

ENV RUST_TARGET=aarch64-unknown-linux-gnu

FROM platform-${TARGETARCH} AS final

WORKDIR /app
COPY /target/$RUST_TARGET/release/golem-component-service ./
COPY /golem-component-service/config/component-service.toml ./config/component-service.toml
COPY /golem-component-service/db ./db

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

EXPOSE 8080
EXPOSE 9090

ENTRYPOINT ["./golem-component-service"]
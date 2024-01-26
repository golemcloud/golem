FROM debian:bookworm-slim

ARG TARGETARCH

FROM base AS platform-amd64

ENV RUST_TARGET=x86_64-unknown-linux-gnu

FROM base AS platform-arm64

ENV RUST_TARGET=aarch64-unknown-linux-gnu

FROM platform-${TARGETARCH} AS final

WORKDIR /app
COPY /target/$RUST_TARGET/release/cloud-server-oss ./
COPY /golem-cloud-server-oss/config/cloud-server.toml ./config/cloud-server.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

EXPOSE 8080
EXPOSE 9090

ENTRYPOINT ["./cloud-server-oss"]
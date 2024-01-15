FROM debian:bookworm-slim

WORKDIR /app
COPY /target/x86_64-unknown-linux-gnu/debug/cloud-server-oss ./
COPY /golem-cloud-server-oss/config/cloud-server.toml ./config/cloud-server.toml

ARG SHARD_MANAGER_HOST
ARG SHARD_MANAGER_PORT
ARG TEMPLATES__STORE__ROOT_PATH

ENV ENVIRONMENT=$ENVIRONMENT \
    RUST_LOG="debug,h2=warn,hyper=warn,tower=warn" \
    WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM__TEMPLATES__STORE__ROOT_PATH=$TEMPLATES__STORE__ROOT_PATH \
    GOLEM__ROUTING_TABLE__HOST=$SHARD_MANAGER_HOST \
    GOLEM__ROUTING_TABLE__PORT=$SHARD_MANAGER_PORT \
    GOLEM__GRPC_PORT=9090 \
    GOLEM__HTTP_PORT=8080

EXPOSE 8080
EXPOSE 9090

ENTRYPOINT ["./cloud-server-oss"]
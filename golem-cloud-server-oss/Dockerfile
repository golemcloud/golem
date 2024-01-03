FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y unzip curl \
    && PROTOC_ZIP=protoc-3.15.0-linux-x86_64.zip \
    && curl -OL https://github.com/google/protobuf/releases/download/v3.15.0/$PROTOC_ZIP \
    && unzip -o $PROTOC_ZIP -d /usr/local bin/protoc \
    && rm -f $PROTOC_ZIP

WORKDIR /app
COPY /target/debug/cloud-server-oss ./
COPY /golem-cloud-server-oss/config/cloud-server.toml ./config/cloud-server.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

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

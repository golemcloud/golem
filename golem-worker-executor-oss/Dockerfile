FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y unzip curl\
    && PROTOC_ZIP=protoc-3.15.0-linux-x86_64.zip \
    && curl -OL https://github.com/google/protobuf/releases/download/v3.15.0/$PROTOC_ZIP \
    && unzip -o $PROTOC_ZIP -d /usr/local bin/protoc \
    && rm -f $PROTOC_ZIP

WORKDIR /app
COPY /target/x86_64-unknown-linux-gnu/debug/worker-executor-oss ./
COPY /golem-worker-executor-oss/config/worker-executor-local.toml ./config/worker-executor.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

ARG ENVIRONMENT
ARG CLOUD_SERVICE_HOST
ARG SHARD_MANAGER_HOST
ARG SHARD_MANAGER_PORT

ENV ENVIRONMENT=$ENVIRONMENT \
    RUST_LOG=info \
    WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM__REDIS__PORT=6379 \
    GOLEM__TEMPLATE_SERVICE__CONFIG__HOST=$CLOUD_SERVICE_HOST \
    GOLEM__TEMPLATE_SERVICE__CONFIG__PORT=9090 \
    GOLEM__COMPILED_TEMPLATE_SERVICE__CONFIG__OBJECT_PREFIX="" \
    GOLEM__PORT=9000 \
    GOLEM__HTTP_PORT=8082 \
    GOLEM__SHARD_MANAGER_SERVICE__CONFIG__HOST=$SHARD_MANAGER_HOST \
    GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT=$SHARD_MANAGER_PORT \
    GOLEM__ENABLE_JSON_LOG=true

EXPOSE 9000

ENTRYPOINT ["./worker-executor-oss"]

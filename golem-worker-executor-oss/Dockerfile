#FROM 242553885714.dkr.ecr.us-east-1.amazonaws.com/golem-rust-base-image-dev:626595d as build
FROM rust:1.74.0-bullseye as build

RUN apt-get update && apt-get install -y unzip \
    && PROTOC_ZIP=protoc-3.15.0-linux-x86_64.zip \
    && curl -OL https://github.com/google/protobuf/releases/download/v3.15.0/$PROTOC_ZIP \
    && unzip -o $PROTOC_ZIP -d /usr/local bin/protoc \
    && rm -f $PROTOC_ZIP

WORKDIR /app
COPY . /app

WORKDIR /app/worker-executor

# Ensure base-image dependencies are compatible with worker-executor dependencies
#RUN chmod +x pre-baked-image-check.sh
#RUN set -e && ./pre-baked-image-check.sh

# All the target builds will not within worker-executor/target directory
# but in /baseApp/ directory such that its a union of what's in base package ++ new dependencies in worker-executor
RUN cargo build -p worker-executor --release --target-dir=/baseApp/target

# This is the lightest image we could find
# in the second stage. Good to make it even lighter!
# The final size of the image depends on this stage of the build

FROM segment/chamber:2 AS chamber

FROM debian:bullseye-slim

WORKDIR app/

COPY --from=chamber /chamber /bin/chamber
COPY --from=build /baseApp/target/release/worker-executor .
COPY --from=build /app/worker-executor/config/worker-executor.toml ./config/worker-executor.toml

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

COPY worker-executor/worker-executor-wrapper.sh worker-executor-wrapper.sh
RUN chmod +x worker-executor-wrapper.sh
EXPOSE 9000

ENTRYPOINT ["./worker-executor-wrapper.sh"]

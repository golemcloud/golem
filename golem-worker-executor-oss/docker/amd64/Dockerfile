FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef as builder

COPY --from=planner /app/recipe.json recipe.json
COPY ./Cargo.toml ./Cargo.lock ./
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN apt-get update && apt-get install -y unzip \
    && PROTOC_ZIP=protoc-3.15.0-linux-x86_64.zip \
    && curl -OL https://github.com/google/protobuf/releases/download/v3.15.0/$PROTOC_ZIP \
    && unzip -o $PROTOC_ZIP -d /usr/local bin/protoc \
    && rm -f $PROTOC_ZIP
RUN cargo build -p golem-worker-executor-oss --release --bin worker-executor-oss

FROM debian:bookworm-slim

WORKDIR /app
COPY --from=builder /app/target/release/worker-executor-oss ./
COPY --from=builder /app/golem-worker-executor-oss/config/worker-executor-local.toml ./config/worker-executor.toml

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
    GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_ATTEMPTS="5" \
    GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MIN_DELAY="100ms" \
    GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_DELAY="2s" \
    GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MULTIPLIER="2" \
    GOLEM__SHARD_MANAGER_SERVICE__TYPE="Grpc" \
    GOLEM__ENABLE_JSON_LOG=true

EXPOSE 9000

ENTRYPOINT ["./worker-executor-oss"]

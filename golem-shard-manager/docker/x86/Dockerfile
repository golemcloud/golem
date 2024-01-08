FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef as builder

RUN apt-get update && apt-get install -y unzip \
    && PROTOC_ZIP=protoc-3.15.0-linux-x86_64.zip \
    && curl -OL https://github.com/google/protobuf/releases/download/v3.15.0/$PROTOC_ZIP \
    && unzip -o $PROTOC_ZIP -d /usr/local bin/protoc \
    && rm -f $PROTOC_ZIP

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo chef cook --release
RUN cargo build -p golem-shard-manager

FROM debian:bookworm-slim

WORKDIR /app
COPY --from=builder /app/target/debug/golem-shard-manager ./
COPY --from=builder /app/golem-shard-manager/config/shard-manager.toml ./config/shard-manager.toml

ARG REDIS_HOST

ENV WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM__REDIS__HOST=$REDIS_HOST \
    GOLEM__REDIS__PORT=6379 \
    GOLEM__ENABLE_JSON_LOG=true \
    GOLEM__HTTP_PORT=8081 \
    GOLEM_SHARD_MANAGER_PORT=9001

EXPOSE 8081
EXPOSE 9001

ENTRYPOINT ["./golem-shard-manager"]


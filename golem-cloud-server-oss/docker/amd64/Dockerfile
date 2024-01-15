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
RUN cargo build -p cloud-server-oss --release --bin cloud-server-oss

FROM debian:bookworm-slim AS runtime

WORKDIR /app
COPY --from=builder /app/target/release/cloud-server-oss /usr/local/bin
COPY --from=builder /app/golem-cloud-server-oss/config/cloud-server.toml ./config/cloud-server.toml

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

ENTRYPOINT ["/usr/local/bin/cloud-server-oss"]

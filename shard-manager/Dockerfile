FROM rust:1.74.0-bullseye as build

RUN apt-get update && apt-get install -y unzip \
    && PROTOC_ZIP=protoc-3.15.0-linux-x86_64.zip \
    && curl -OL https://github.com/google/protobuf/releases/download/v3.15.0/$PROTOC_ZIP \
    && unzip -o $PROTOC_ZIP -d /usr/local bin/protoc \
    && rm -f $PROTOC_ZIP

WORKDIR /app
COPY . /app

RUN cargo build -p shard-manager --release --target-dir=/baseApp/target

FROM debian:bullseye-slim

WORKDIR app/

COPY --from=build /baseApp/target/release/shard-manager .
COPY --from=build /app/shard-manager/config/shard-manager.toml ./config/shard-manager.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

ARG SHARD_MANAGER_PORT
ARG REDIS_DATABASE
ARG REDIS_KEY_PREFIX

ENV WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM_SHARD_MANAGER_PORT=$SHARD_MANAGER_PORT \
    GOLEM__REDIS__PORT=6379 \
    GOLEM__REDIS__DATABASE=$REDIS_DATABASE \
    GOLEM__REDIS__KEY_PREFIX=$REDIS_KEY_PREFIX \
    GOLEM__ENABLE_JSON_LOG=true \
    GOLEM__HTTP_PORT=8080

EXPOSE 8080

ENTRYPOINT ["./shard-manager"]


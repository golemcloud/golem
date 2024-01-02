# Make sure you run docker build from the root directory of golem-services. See README.md
FROM rust:1.74.0-bullseye as build

RUN apt-get update && apt-get install -y unzip \
    && PROTOC_ZIP=protoc-3.15.0-linux-x86_64.zip \
    && curl -OL https://github.com/google/protobuf/releases/download/v3.15.0/$PROTOC_ZIP \
    && unzip -o $PROTOC_ZIP -d /usr/local bin/protoc \
    && rm -f $PROTOC_ZIP

WORKDIR /app
COPY . /app

WORKDIR /app/golem-cloud-server-oss

RUN cargo build -p cloud-server-oss --release --target-dir=/baseApp/target

FROM debian:bullseye-slim

WORKDIR app/

COPY --from=build /baseApp/target/release/cloud-server-oss .
COPY --from=build /app/golem-cloud-server-oss/config/cloud-server.toml ./config/cloud-server.toml

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

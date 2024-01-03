FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y unzip curl \
    && PROTOC_ZIP=protoc-3.15.0-linux-x86_64.zip \
    && curl -OL https://github.com/google/protobuf/releases/download/v3.15.0/$PROTOC_ZIP \
    && unzip -o $PROTOC_ZIP -d /usr/local bin/protoc \
    && rm -f $PROTOC_ZIP

WORKDIR /app
COPY /target/x86_64-unknown-linux-gnu/debug/golem-shard-manager ./
COPY /golem-shard-manager/config/shard-manager.toml ./config/shard-manager.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

ARG REDIS_HOST

ENV WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM__REDIS__HOST=$REDIS_HOST \
    GOLEM__REDIS__PORT=6379 \
    GOLEM__ENABLE_JSON_LOG=true \
    GOLEM__HTTP_PORT=8081

EXPOSE 8081

ENTRYPOINT ["./golem-shard-manager"]


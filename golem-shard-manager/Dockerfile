FROM rust:1.74.0-bullseye as build

RUN apt-get update && apt-get install -y unzip \
    && PROTOC_ZIP=protoc-3.15.0-linux-x86_64.zip \
    && curl -OL https://github.com/google/protobuf/releases/download/v3.15.0/$PROTOC_ZIP \
    && unzip -o $PROTOC_ZIP -d /usr/local bin/protoc \
    && rm -f $PROTOC_ZIP

WORKDIR /app
COPY . /app

RUN cargo build -p golem-shard-manager --release --target-dir=/baseApp/target

FROM debian:bullseye-slim

WORKDIR app/

COPY --from=build /baseApp/target/release/golem-shard-manager .
COPY --from=build /app/golem-shard-manager/config/shard-manager.toml ./config/shard-manager.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

ENV WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM__REDIS__PORT=6379 \
    GOLEM__ENABLE_JSON_LOG=true \
    GOLEM__HTTP_PORT=8081

EXPOSE 8081

ENTRYPOINT ["./golem-shard-manager"]


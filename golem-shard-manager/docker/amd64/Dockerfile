FROM debian:bookworm-slim

WORKDIR /app
COPY /target/x86_64-unknown-linux-gnu/debug/golem-shard-manager ./
COPY /golem-shard-manager/config/shard-manager.toml ./config/shard-manager.toml

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


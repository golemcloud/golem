FROM segment/chamber:2 AS chamber

FROM debian:bookworm-slim

COPY --from=chamber /chamber /bin/chamber

WORKDIR /app

COPY /target/x86_64-unknown-linux-gnu/release/golem-shard-manager ./
COPY /cloud-shard-manager/config/shard-manager.toml ./config/shard-manager.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

ARG ENVIRONMENT
ARG SHARD_MANAGER_PORT
ARG REDIS_DATABASE
ARG REDIS_KEY_PREFIX

ENV ENVIRONMENT=$ENVIRONMENT \
    RUST_LOG="debug,h2=warn,hyper=warn,tower=warn" \
    WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM_SHARD_MANAGER_PORT=$SHARD_MANAGER_PORT \
    GOLEM__REDIS__PORT=6379 \
    GOLEM__REDIS__DATABASE=$REDIS_DATABASE \
    GOLEM__REDIS__KEY_PREFIX=$REDIS_KEY_PREFIX \
    GOLEM__ENABLE_JSON_LOG=true \
    GOLEM__HTTP_PORT=8080

EXPOSE 8080

RUN chmod +x golem-shard-manager

COPY cloud-shard-manager/cloud-shard-manager-wrapper.sh cloud-shard-manager-wrapper.sh
RUN chmod +x cloud-shard-manager-wrapper.sh
ENTRYPOINT ["./cloud-shard-manager-wrapper.sh"]


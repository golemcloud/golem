FROM segment/chamber:2 AS chamber

FROM debian:bookworm-slim

COPY --from=chamber /chamber /bin/chamber

WORKDIR /app
COPY /target/x86_64-unknown-linux-gnu/release/cloud-worker-service ./
COPY /cloud-worker-service/config/worker-service.toml ./config/worker-service.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

ARG ENVIRONMENT
ARG WORKSPACE
ARG REDIS_DATABASE
ARG REDIS_KEY_PREFIX
ARG COMPONENT_SERVICE_HOST
ARG SHARD_MANAGER_HOST

ENV ENVIRONMENT=$ENVIRONMENT
ENV AWS_REGION="us-east-1"
ENV GOLEM__COMPONENT_SERVICE__HOST=$COMPONENT_SERVICE_HOST
ENV GOLEM__COMPONENT_SERVICE__PORT=9090
ENV GOLEM__ROUTING_TABLE__HOST=$SHARD_MANAGER_HOST
ENV GOLEM__ROUTING_TABLE__PORT=9001
ENV GOLEM__ENABLE_JSON_LOG=true
ENV GOLEM__ENVIRONMENT=$ENVIRONMENT
ENV GOLEM__WORKSPACE=$WORKSPACE
ENV GOLEM__REDIS__PORT=6379
ENV GOLEM__REDIS__DATABASE=$REDIS_DATABASE
ENV GOLEM__REDIS__KEY_PREFIX=$REDIS_KEY_PREFIX
ENV RUST_LOG="debug,actix_web=warn,h2=warn,hyper=warn,tower=warn"
ENV RUST_BACKTRACE=1

EXPOSE 9000
EXPOSE 9001
RUN chmod +x cloud-worker-service

COPY cloud-worker-service/cloud-worker-service-wrapper.sh cloud-worker-service-wrapper.sh
RUN chmod +x cloud-worker-service-wrapper.sh
ENTRYPOINT ["./cloud-worker-service-wrapper.sh"]


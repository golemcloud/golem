FROM segment/chamber:2 AS chamber

FROM debian:bookworm-slim

COPY --from=chamber /chamber /bin/chamber

WORKDIR /app
COPY /target/x86_64-unknown-linux-gnu/release/cloud-component-service ./
COPY /cloud-component-service/config/component-service.toml ./config/component-service.toml
COPY /cloud-component-service/db ./db

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

ARG ENVIRONMENT
ARG WORKSPACE
ARG COMPONENT_STORE_OBJECT_PREFIX
ARG COMPONENT_STORE_BUCKET_NAME
ARG COMPONENT_COMPILATION_SERVICE_HOST
ARG CLOUD_SERVICE_HOST

ENV ENVIRONMENT=$ENVIRONMENT \
    RUST_LOG="debug,h2=warn,hyper=warn,tower=warn" \
    WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM__ENVIRONMENT=$ENVIRONMENT \
    GOLEM__WORKSPACE=$WORKSPACE \
    GOLEM__DB__CONFIG__SCHEMA=$WORKSPACE \
    GOLEM__CLOUD_SERVICE__HOST=$CLOUD_SERVICE_HOST \
    GOLEM__CLOUD_SERVICE__PORT=9090 \
    GOLEM__COMPONENT_STORE__TYPE="S3" \
    GOLEM__COMPONENT_STORE__CONFIG__OBJECT_PREFIX=$COMPONENT_STORE_OBJECT_PREFIX \
    GOLEM__COMPONENT_STORE__CONFIG__BUCKET_NAME=$COMPONENT_STORE_BUCKET_NAME \
    GOLEM__COMPILATION__TYPE="Enabled" \
    GOLEM__COMPILATION__CONFIG__HOST=$COMPONENT_COMPILATION_SERVICE_HOST \
    GOLEM__COMPILATION__CONFIG__PORT=9090 \
    GOLEM__GRPC_PORT=9090 \
    GOLEM__HTTP_PORT=8083 \
    GOLEM__ENABLE_JSON_LOG=true

RUN chmod +x cloud-component-service

COPY cloud-component-service/cloud-component-service-wrapper.sh cloud-component-service-wrapper.sh
RUN chmod +x cloud-component-service-wrapper.sh

EXPOSE 8083
EXPOSE 9090

ENTRYPOINT ["./cloud-component-service-wrapper.sh"]

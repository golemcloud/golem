FROM segment/chamber:2 AS chamber

FROM debian:bookworm-slim

COPY --from=chamber /chamber /bin/chamber

WORKDIR /app

COPY /target/x86_64-unknown-linux-gnu/release/cloud-component-compilation-service ./
COPY /cloud-component-compilation-service/config/component-compilation-service.toml ./config/component-compilation-service.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

ARG ENVIRONMENT
ARG COMPONENT_SERVICE_HOST
ARG COMPONENT_STORE_OBJECT_PREFIX

ENV ENVIRONMENT=$ENVIRONMENT \
    RUST_LOG="info,h2=warn,hyper=warn,tower=warn" \
    WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM__COMPONENT_SERVICE__HOST=$COMPONENT_SERVICE_HOST \
    GOLEM__COMPONENT_SERVICE__PORT=9090 \
    GOLEM__BLOB_STORAGE__TYPE="S3" \
    GOLEM__BLOB_STORAGE__CONFIG__REGION=us-east-1 \
    GOLEM__BLOB_STORAGE__CONFIG__OBJECT_PREFIX="${COMPONENT_STORE_OBJECT_PREFIX}" \
    GOLEM__BLOB_STORAGE__CONFIG__USE_MINIO_CREDENTIALS=false \
    GOLEM__BLOB_STORAGE__CONFIG__RETRIES__MAX_ATTEMPTS=3 \
    GOLEM__BLOB_STORAGE__CONFIG__RETRIES__MIN_DELAY="100ms" \
    GOLEM__BLOB_STORAGE__CONFIG__RETRIES__MAX_DELAY="1s" \
    GOLEM__BLOB_STORAGE__CONFIG__RETRIES__MULTIPLIER=3 \
    GOLEM__TRACING__STDOUT__JSON=true \
    GOLEM__GRPC_PORT=9090 \
    GOLEM__HTTP_PORT=8080

EXPOSE 8080
EXPOSE 9090

RUN chmod +x cloud-component-compilation-service

COPY cloud-component-compilation-service/cloud-component-compilation-service-wrapper.sh cloud-component-compilation-service-wrapper.sh
RUN chmod +x cloud-component-compilation-service-wrapper.sh
ENTRYPOINT ["./cloud-component-compilation-service-wrapper.sh"]


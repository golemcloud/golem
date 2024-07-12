FROM segment/chamber:2 AS chamber

FROM debian:bookworm-slim

COPY --from=chamber /chamber /bin/chamber

WORKDIR /app
COPY /target/x86_64-unknown-linux-gnu/release/cloud-service ./
COPY /cloud-service/config/cloud-service.toml ./config/cloud-service.toml
COPY /cloud-service/db ./db

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

ARG ENVIRONMENT
ARG WORKSPACE

ENV ENVIRONMENT=$ENVIRONMENT \
    RUST_LOG="debug,h2=warn,hyper=warn,tower=warn" \
    WASMTIME_BACKTRACE_DETAILS=1 \
    RUST_BACKTRACE=1 \
    GOLEM__ENVIRONMENT=$ENVIRONMENT \
    GOLEM__WORKSPACE=$WORKSPACE \
    GOLEM__DB__CONFIG__SCHEMA=$WORKSPACE \
    GOLEM__DB__CONFIG__PORT=5432 \
    GOLEM__GRPC_PORT=9090 \
    GOLEM__HTTP_PORT=8080 \
    GOLEM__TRACING__STDOUT__JSON=true \
    GOLEM__PLANS__DEFAULT__PLAN_ID="80b56370-1ed4-4d90-864b-e8809641995d" \
    GOLEM__OAUTH2__GITHUB_CLIENT_ID="c57138f3680c74ab39d4"

RUN chmod +x cloud-service

COPY cloud-service/cloud-service-wrapper.sh cloud-service-wrapper.sh
RUN chmod +x cloud-service-wrapper.sh

EXPOSE 8080
EXPOSE 9090

ENTRYPOINT ["./cloud-service-wrapper.sh"]

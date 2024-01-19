FROM debian:bookworm-slim

WORKDIR /app
COPY /target/x86_64-unknown-linux-gnu/release/worker-executor-oss ./
COPY /golem-worker-executor-oss/config/worker-executor.toml ./config/worker-executor.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

EXPOSE 9000

ENTRYPOINT ["./worker-executor-oss"]
FROM debian:bookworm-slim

ARG TARGETPLATFORM

WORKDIR /app
COPY /target/${TARGETPLATFORM}/release/golem-shard-manager ./
COPY /golem-shard-manager/config/shard-manager.toml ./config/shard-manager.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

EXPOSE 8081
EXPOSE 9001

ENTRYPOINT ["./golem-shard-manager"]


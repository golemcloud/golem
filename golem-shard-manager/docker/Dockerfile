FROM debian:bookworm-slim

ARG TARGETARCH

WORKDIR /app

COPY /golem-shard-manager/config/shard-manager.toml ./config/shard-manager.toml
COPY /target/$TARGETARCH/release/golem-shard-manager ./

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

EXPOSE 8081
EXPOSE 9001

ENTRYPOINT ["./golem-shard-manager"]


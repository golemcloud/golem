FROM debian:bookworm-slim

WORKDIR /app
COPY /target/x86_64-unknown-linux-gnu/debug/cloud-server-oss ./
COPY /golem-cloud-server-oss/config/cloud-server.toml ./config/cloud-server.toml

RUN apt-get update && apt-get install -y libssl-dev
RUN apt-get update && apt-get install -y ca-certificates
RUN update-ca-certificates

EXPOSE 8080
EXPOSE 9090

ENTRYPOINT ["./cloud-server-oss"]
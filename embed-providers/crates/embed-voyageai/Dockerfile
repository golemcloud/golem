FROM rust:1.75-slim as builder

WORKDIR /usr/src/app
COPY . .
RUN cargo init
RUN cargo add tokio reqwest serde async-trait once_cell
RUN cargo build --release

FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/app/target/release/embed-voyageai /usr/local/bin/

ENV VOYAGE_API_KEY=""
EXPOSE 8080
CMD ["embed-voyageai"]
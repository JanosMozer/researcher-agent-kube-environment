FROM rust:1.78-slim as builder
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
RUN cargo build --release --bin controller
RUN cargo build --release --bin shim

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/controller /usr/local/bin/controller
COPY --from=builder /app/target/release/shim /usr/local/bin/shim

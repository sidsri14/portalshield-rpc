# Build stage
FROM rust:1.80-slim-bookworm as builder

WORKDIR /usr/src/portalshield-rpc
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /usr/src/portalshield-rpc/target/release/portalshield-rpc /usr/local/bin/portalshield-rpc
COPY config.example.json /app/config.json

EXPOSE 9944

ENTRYPOINT ["portalshield-rpc"]
CMD ["--config", "/app/config.json"]

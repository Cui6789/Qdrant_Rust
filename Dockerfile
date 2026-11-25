# syntax=docker/dockerfile:1

# ---- Build stage -----------------------------------------------------------
# Pinned toolchain; the workspace MSRV is 1.92, but 1.100 is the verified build.
FROM rust:1.100-bookworm AS builder

WORKDIR /app

# Manifests first so dependency compilation is cached independently of sources.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Default features include `local-embedding` (fastembed/ONNX Runtime).
# The first build downloads crates and the prebuilt ONNX Runtime, so it needs
# network access.
RUN cargo build --release -p kb-server

# ---- Runtime stage ---------------------------------------------------------
FROM debian:bookworm-slim

# ca-certificates: Hugging Face model download over HTTPS.
# libstdc++6 and libgomp1: C++/OpenMP runtime libraries used by the
# statically linked ONNX Runtime bundled with fastembed.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libstdc++6 \
        libgomp1 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# `CMD ["kb-server"]` resolves through PATH, so install the binary there.
COPY --from=builder /app/target/release/kb-server /usr/local/bin/kb-server

# Default configuration. `config/config.toml` is resolved relative to the
# working directory (`/app`); docker-compose overrides endpoints and the model
# cache through KB_* environment variables.
COPY config/config.example.toml /app/config/config.toml

EXPOSE 8080

CMD ["kb-server"]
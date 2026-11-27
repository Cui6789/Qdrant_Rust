# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.100

# ---- Shared Rust and Trunk toolchain ---------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS toolchain

ARG TRUNK_VERSION=0.22.2
ARG TRUNK_SHA256=4a90216d93c553f3d8b7401fce0dee588e0fb1eb1c6a379ebac81fabe820f25b

ENV CARGO_TERM_COLOR=always \
    CARGO_INCREMENTAL=0 \
    TRUNK_SKIP_VERSION_CHECK=true

RUN rustup target add wasm32-unknown-unknown

# Trunk 0.22.2 has Rust MSRV 1.96.1, while the project is built with Rust 1.100.
# The archive checksum makes the prebuilt binary reproducible.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl; \
    rm -rf /var/lib/apt/lists/*; \
    curl --retry 5 --retry-delay 2 -fsSL \
      "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-musl.tar.gz" \
      -o /tmp/trunk.tar.gz; \
    echo "${TRUNK_SHA256}  /tmp/trunk.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/trunk.tar.gz -C /usr/local/bin trunk; \
    chmod 0755 /usr/local/bin/trunk; \
    rm /tmp/trunk.tar.gz; \
    trunk --version

# ---- Dashboard WASM workbench ----------------------------------------------
FROM toolchain AS ui-builder

ARG WASM_BINDGEN_VERSION=0.2.130
ARG WASM_BINDGEN_SHA256=9687f3bb551b9f66f37ed32a40d7913a00f7962db653054e56857bc306c3227f

# Match the wasm-bindgen crate version in the UI lockfile. Preinstall the CLI
# so the build does not depend on Trunk's separate tool-download timeout.
RUN set -eux; \
    archive="wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl"; \
    curl --retry 5 --retry-all-errors --connect-timeout 15 --max-time 180 -fsSL \
      "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/${archive}.tar.gz" \
      -o /tmp/wasm-bindgen.tar.gz; \
    echo "${WASM_BINDGEN_SHA256}  /tmp/wasm-bindgen.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/wasm-bindgen.tar.gz --strip-components=1 -C /usr/local/bin "${archive}/wasm-bindgen"; \
    rm /tmp/wasm-bindgen.tar.gz; \
    wasm-bindgen --version

WORKDIR /app
COPY crates/kb-dashboard-ui ./crates/kb-dashboard-ui
COPY crates/kb-dashboard-types ./crates/kb-dashboard-types

WORKDIR /app/crates/kb-dashboard-ui
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=shared \
    --mount=type=cache,target=/app/crates/kb-dashboard-ui/target,sharing=shared \
    trunk build --release --locked --minify=false

# ---- Rust workspace ---------------------------------------------------------
FROM rust:${RUST_VERSION}-trixie AS rust-builder

ENV CARGO_TERM_COLOR=always \
    CARGO_INCREMENTAL=0

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY config ./config

# The target cache is mounted only for the build; copy the server into the image
# layer for the runtime stage.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=shared \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=shared \
    --mount=type=cache,target=/app/target,sharing=shared \
    cargo build --release --locked \
      -p kb-server \
      --features kb-server/local-embedding,kb-server/local-rerank \
    && mkdir -p /out \
    && cp target/release/kb-server /out/

# ---- Development/runtime image ---------------------------------------------
# The ONNX Runtime archive needs the newer glibc and libstdc++ in Trixie.
FROM debian:trixie-slim AS runtime

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
      libgomp1 \
      libstdc++6 \
      tini; \
    rm -rf /var/lib/apt/lists/*; \
    mkdir -p /app/config /app/crates/kb-dashboard-ui/dist /data/models /data/settings

WORKDIR /app

COPY --from=rust-builder /out/kb-server /usr/local/bin/kb-server
COPY --from=rust-builder /app/config/config.example.toml /app/config/config.toml
COPY --from=ui-builder /app/crates/kb-dashboard-ui/dist /app/crates/kb-dashboard-ui/dist

ENV KB_CONFIG=/app/config/config.toml \
    DASHBOARD_UI_DIR=/app/crates/kb-dashboard-ui/dist \
    RUST_LOG=info,kb_core=debug

EXPOSE 8080 8090

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["kb-server"]

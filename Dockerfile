# syntax=docker/dockerfile:1
# tg-sync — Production-ready Docker image
# Multi-stage build: minimal final image, reproducible releases

# ─────────────────────────────────────────────────────────────────────────────
# Stage 1: Planner — cache dependency layer separately from app code
# ─────────────────────────────────────────────────────────────────────────────
FROM rust:1.83-bookworm AS planner
WORKDIR /build
RUN cargo install cargo-chef
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Compute recipe for dependency caching
RUN cargo chef prepare --recipe-path recipe.json

# ─────────────────────────────────────────────────────────────────────────────
# Stage 2: Builder — compile dependencies (cached) then application
# ─────────────────────────────────────────────────────────────────────────────
FROM rust:1.83-bookworm AS builder
WORKDIR /build

# Install build deps (OpenSSL for transitive native-tls; ca-certificates)
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy recipe and build dependencies only (cache layer)
COPY --from=planner /build/recipe.json .
RUN cargo chef cook --release --recipe-path recipe.json

# Copy source and build application
COPY . .
RUN cargo build --release

# ─────────────────────────────────────────────────────────────────────────────
# Stage 3: Runtime — minimal production image
# ─────────────────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# Install runtime deps only (ca-certificates for HTTPS; libssl for native-tls)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for security
RUN groupadd -r tgsync && useradd -r -g tgsync tgsync

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/target/release/tg-sync /usr/local/bin/tg-sync

# Create data dirs; tg-sync expects ./data and session in CWD
RUN mkdir -p /app/data/media /app/data/reports /app/session && \
    chown -R tgsync:tgsync /app

# Entrypoint: fix volume permissions (named vols mount as root) then run as tgsync
COPY <<'SCRIPT' /entrypoint.sh
#!/bin/sh
chown -R tgsync:tgsync /app/session /app/data 2>/dev/null || true
exec runuser -u tgsync -- /usr/local/bin/tg-sync "$@"
SCRIPT
RUN chmod +x /entrypoint.sh

# Default paths (override via env)
ENV TG_SYNC_DATA_DIR=/app/data
ENV TG_SYNC_SESSION_PATH=/app/session/session.db

# Interactive TUI requires TTY; run with: docker run -it ...
ENTRYPOINT ["/entrypoint.sh"]

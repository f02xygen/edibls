# ==========================================
# 1. Builder Stage: Compile Rust application
# ==========================================
FROM rust:bookworm AS builder

WORKDIR /usr/src/edibls

# Cache dependencies by building a dummy crate first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src

# Copy actual source code and build release binary
COPY src ./src
RUN touch src/main.rs && cargo build --release

# ==========================================
# 2. Local Telegram Bot API Stage
# ==========================================
FROM aiogram/telegram-bot-api:latest AS tg-api

# ==========================================
# 3. Runtime Stage: Python + FFmpeg + Deno/Node + yt-dlp + yandex-music-downloader + telegram-bot-api
# ==========================================
FROM python:3.12-slim-bookworm

ENV DEBIAN_FRONTEND=noninteractive \
    PYTHONUNBUFFERED=1 \
    RUST_LOG=info

WORKDIR /app

# Install system dependencies: ffmpeg, ca-certificates, curl, unzip, nodejs
RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg \
    ca-certificates \
    curl \
    unzip \
    nodejs \
    && rm -rf /var/lib/apt/lists/*

# Install Deno (for yt-dlp JavaScript challenge solving)
RUN curl -fsSL https://deno.land/install.sh | sh \
    && mv /root/.deno/bin/deno /usr/local/bin/deno || true

# Install fresh yt-dlp (with curl-cffi for TikTok/Cloudflare TLS impersonation) and yandex-music-downloader
RUN pip install --no-cache-dir --upgrade \
    "yt-dlp[default,curl-cffi]" \
    https://github.com/llistochek/yandex-music-downloader/archive/main.zip

# Copy local Telegram Bot API server binary and its musl runtime libraries (for 2GB uploads)
COPY --from=tg-api /usr/local/bin/telegram-bot-api /usr/local/bin/telegram-bot-api
COPY --from=tg-api /lib/ld-musl-*.so.1 /lib/
COPY --from=tg-api \
    /usr/lib/libssl.so.3 \
    /usr/lib/libcrypto.so.3 \
    /usr/lib/libz.so.1* \
    /usr/lib/libstdc++.so.6* \
    /usr/lib/libgcc_s.so.1 \
    /usr/lib/

# Copy the compiled edibls binary from builder
COPY --from=builder /usr/src/edibls/target/release/edibls /usr/local/bin/edibls

# Run as a dedicated non-root user (with write access to Python site-packages/bin for yt-dlp auto-updates)
RUN useradd -m -u 1000 appuser \
    && chown -R appuser:appuser /app /usr/local/lib/python3.12 /usr/local/bin
USER appuser

ENTRYPOINT ["/bin/sh", "-c", "pip install -q --no-cache-dir -U 'yt-dlp[default,curl-cffi]' || true; exec /usr/local/bin/edibls"]

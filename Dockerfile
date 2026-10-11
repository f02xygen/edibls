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
# 2. Runtime Stage: Python + FFmpeg + Deno/Node + yt-dlp + yandex-music-downloader
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

# Copy the compiled edibls binary from builder
COPY --from=builder /usr/src/edibls/target/release/edibls /usr/local/bin/edibls

# Run as a dedicated non-root user (with write access to Python site-packages/bin for yt-dlp auto-updates)
RUN useradd -m -u 1000 appuser \
    && chown -R appuser:appuser /app /usr/local/lib/python3.12 /usr/local/bin
USER appuser

ENTRYPOINT ["/bin/sh", "-c", "pip install -q --no-cache-dir -U 'yt-dlp[default,curl-cffi]' || true; exec /usr/local/bin/edibls"]

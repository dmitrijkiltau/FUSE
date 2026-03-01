# syntax=docker/dockerfile:1.7

FROM rust:slim-trixie AS builder
WORKDIR /workspace

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates \
       curl \
       libsqlite3-dev \
       pkg-config \
       tar \
  && rm -rf /var/lib/apt/lists/*

ARG FUSE_CLI_URL=https://github.com/dmitrijkiltau/FUSE/releases/download/v0.7.0/fuse-cli-linux-x64.tar.gz

# Use prebuilt CLI artifact; do not build fuse from source in Docker.
RUN curl -fsSL "$FUSE_CLI_URL" -o /tmp/fuse-cli-linux-x64.tar.gz \
  && tar -xzf /tmp/fuse-cli-linux-x64.tar.gz -C /tmp fuse \
  && install -m 0755 /tmp/fuse /usr/local/bin/fuse \
  && rm -f /tmp/fuse /tmp/fuse-cli-linux-x64.tar.gz

# Build docs assets + AOT inside container with prebuilt fuse CLI.
COPY Cargo.toml Cargo.lock rust-toolchain.toml /workspace/
COPY crates /workspace/crates
# fuse build resolves repo root by checking scripts/cargo_env.sh presence.
COPY scripts/cargo_env.sh /workspace/scripts/cargo_env.sh
COPY docs /workspace/docs

ENV CARGO_TARGET_DIR=/workspace/tmp/fuse-target
ENV RUSTC_TMPDIR=/workspace/tmp/fuse-target/tmp
ENV TMPDIR=/workspace/tmp/fuse-target/tmp
ENV TMP=/workspace/tmp/fuse-target/tmp
ENV TEMP=/workspace/tmp/fuse-target/tmp
ENV CARGO_INCREMENTAL=0

RUN mkdir -p /workspace/tmp/fuse-target /workspace/tmp/fuse-target/tmp \
  && fuse build --manifest-path docs --release

FROM debian:trixie-slim@sha256:1d3c811171a08a5adaa4a163fbafd96b61b87aa871bbc7aa15431ac275d3d430 AS runtime
WORKDIR /app/docs

# Runtime deps: sqlite + curl for healthcheck.
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates \
       libsqlite3-0 \
       curl \
  && rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/docs/.fuse/build/program.aot /app/docs/docs-aot
COPY --from=builder /workspace/docs/site /app/docs/site
COPY --from=builder /workspace/docs/assets /app/docs/assets

# Run as non-root.
RUN chmod +x /app/docs/docs-aot \
 && useradd -r -u 10001 appuser \
 && chown -R appuser:appuser /app/docs
USER appuser

ENV FUSE_HOST=0.0.0.0
ENV PORT=4080
ENV FUSE_STATIC_DIR=/app/docs
EXPOSE 4080

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD curl -fsS "http://localhost:${PORT}/api/health" || exit 1

ENTRYPOINT ["./docs-aot"]

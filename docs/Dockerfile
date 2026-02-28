# syntax=docker/dockerfile:1.7

FROM rust:slim-bookworm AS builder
WORKDIR /workspace

# System deps for building (sqlite + pkg-config etc.)
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates \
       pkg-config \
       libsqlite3-dev \
  && rm -rf /var/lib/apt/lists/*

# 1) Copy only the files that affect dependency resolution first (better caching)
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY scripts/cargo_env.sh ./scripts/cargo_env.sh
COPY scripts/generate_guide_docs.sh ./scripts/generate_guide_docs.sh
COPY docs ./docs

RUN chmod +x scripts/cargo_env.sh scripts/generate_guide_docs.sh

# 2) Build the fuse CLI from source (multi-arch safe)
#    Use BuildKit cache mounts to speed up rebuilds.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/workspace/target \
    scripts/cargo_env.sh cargo build -p fuse --release --locked \
 && cp -f tmp/fuse-target/release/fuse /usr/local/bin/fuse \
 && /usr/local/bin/fuse --version || true

# 3) Build docs AOT
ENV FUSE_SKIP_GUIDE_DOCS=1
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/workspace/target \
    fuse build --manifest-path docs --release


FROM debian:bookworm-slim AS runtime
WORKDIR /app/docs

# runtime deps: sqlite + curl for healthcheck
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates \
       libsqlite3-0 \
       curl \
  && rm -rf /var/lib/apt/lists/*

# copy only what we need
COPY --from=builder /workspace/docs/.fuse/build/program.aot /app/docs/docs-aot
COPY --from=builder /workspace/docs/site /app/docs/site
COPY --from=builder /workspace/docs/assets /app/docs/assets

# run as non-root (Coolify likes that, security likes that, humans should too)
RUN useradd -r -u 10001 appuser \
 && chown -R appuser:appuser /app/docs
USER appuser

ENV PORT=4080
ENV FUSE_HOST=0.0.0.0
ENV FUSE_STATIC_DIR=/app/docs
EXPOSE 4080

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD curl -fsS "http://localhost:${PORT}/api/health" || exit 1

CMD ["./docs-aot"]

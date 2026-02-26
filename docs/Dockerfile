# syntax=docker/dockerfile:1

FROM rust:slim-bookworm AS builder
WORKDIR /workspace

ARG FUSE_RELEASE_TAG=v0.5.0
ARG FUSE_PLATFORM=linux-x64

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl tar pkg-config libsqlite3-dev \
  && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL "https://github.com/dmitrijkiltau/FUSE/releases/download/${FUSE_RELEASE_TAG}/fuse-cli-${FUSE_PLATFORM}.tar.gz" -o /tmp/fuse-cli.tar.gz \
  && tar -xzf /tmp/fuse-cli.tar.gz -C /usr/local/bin fuse \
  && chmod +x /usr/local/bin/fuse \
  && rm -f /tmp/fuse-cli.tar.gz

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY scripts/cargo_env.sh ./scripts/cargo_env.sh
COPY scripts/generate_guide_docs.sh ./scripts/generate_guide_docs.sh
COPY docs ./docs

RUN chmod +x scripts/cargo_env.sh scripts/generate_guide_docs.sh
ENV FUSE_SKIP_GUIDE_DOCS=1
RUN fuse build --manifest-path docs --aot --release

FROM debian:bookworm-slim AS runtime
WORKDIR /app/docs

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates libsqlite3-0 curl \
  && rm -rf /var/lib/apt/lists/*

# Copy only the AOT binary, static site assets, and SVG dir — not the full source tree
COPY --from=builder /workspace/docs/.fuse/build/program.aot /app/docs/docs-aot
COPY --from=builder /workspace/docs/site /app/docs/site
COPY --from=builder /workspace/docs/assets /app/docs/assets

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD curl -f http://localhost:${PORT:-4080}/api/health || exit 1

EXPOSE 4080
ENV PORT=4080
ENV FUSE_HOST=0.0.0.0
ENV FUSE_STATIC_DIR=/app/docs

CMD ["./docs-aot"]

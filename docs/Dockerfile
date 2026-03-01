# syntax=docker/dockerfile:1.7

FROM debian:bookworm-slim@sha256:74d56e3931e0d5a1dd51f8c8a2466d21de84a271cd3b5a733b803aa91abf4421 AS runtime
WORKDIR /app/docs

# Runtime deps: sqlite + curl for healthcheck.
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates \
       libsqlite3-0 \
       curl \
  && rm -rf /var/lib/apt/lists/*

# Build inputs are prepared outside Docker:
#   ./scripts/generate_guide_docs.sh
#   ./scripts/fuse build --manifest-path docs --release
COPY docs/.fuse/build/program.aot /app/docs/docs-aot
COPY docs/site /app/docs/site
COPY docs/assets /app/docs/assets

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

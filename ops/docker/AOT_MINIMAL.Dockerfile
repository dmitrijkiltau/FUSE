# syntax=docker/dockerfile:1.7

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates \
       libsqlite3-0 \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Build this image from your app package root after `fuse build --release`.
COPY .fuse/build/program.aot /app/app-aot

RUN chmod +x /app/app-aot \
  && useradd -r -u 10001 appuser \
  && chown -R appuser:appuser /app

USER appuser

ENV FUSE_HOST=0.0.0.0
ENV PORT=3000

EXPOSE 3000

ENTRYPOINT ["./app-aot"]

# syntax=docker/dockerfile:1.7

FROM ubuntu:24.04 AS fuse-cli

ARG FUSE_VERSION=v0.9.6

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates \
       curl \
       tar \
  && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL \
    "https://github.com/dmitrijkiltau/fuse/releases/download/${FUSE_VERSION}/fuse-cli-linux-x64.tar.gz" \
    -o /tmp/fuse-cli-linux-x64.tar.gz \
  && mkdir -p /opt/fuse \
  && tar -xzf /tmp/fuse-cli-linux-x64.tar.gz -C /opt/fuse \
  && chmod +x /opt/fuse/fuse

FROM ubuntu:24.04

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
       ca-certificates \
       libsqlite3-0 \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Build this image from the reference-service directory after
# `fuse build --manifest-path examples/reference-service --aot`.
COPY --from=fuse-cli /opt/fuse/fuse /usr/local/bin/fuse
COPY .fuse/build/program.aot /app/app-aot
COPY fuse.toml /app/fuse.toml
COPY src /app/src
COPY assets /app/assets

RUN chmod +x /app/app-aot \
  && chmod +x /usr/local/bin/fuse \
  && useradd -r -u 10001 appuser \
  && mkdir -p /app/data \
  && chown -R appuser:appuser /app

USER appuser

ENV FUSE_HOST=0.0.0.0
ENV APP_PORT=3000
ENV FUSE_DB_URL=sqlite:///app/data/reference-service.db
ENV FUSE_STATIC_DIR=/app

EXPOSE 3000

ENTRYPOINT ["./app-aot"]

# syntax=docker/dockerfile:1.7

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# These files come from the linux release artifact archive (`fuse-aot-linux-x64.tar.gz`).
COPY fuse-aot-demo /app/fuse-aot-demo
COPY AOT_BUILD_INFO.txt /app/AOT_BUILD_INFO.txt
COPY LICENSE /app/LICENSE
COPY README.txt /app/README.txt

RUN chmod +x /app/fuse-aot-demo \
  && useradd -r -u 10001 appuser \
  && chown -R appuser:appuser /app

USER appuser

ENTRYPOINT ["./fuse-aot-demo"]

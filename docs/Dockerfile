# syntax=docker/dockerfile:1

FROM rust:slim-bookworm AS builder
WORKDIR /workspace

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates pkg-config libsqlite3-dev libarchive-tools \
  && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY scripts ./scripts
COPY docs ./docs

RUN chmod +x scripts/cargo_env.sh scripts/generate_guide_docs.sh
RUN ./scripts/cargo_env.sh cargo build -p fuse --release
RUN ./scripts/cargo_env.sh cargo build -p fusec --release --bin fuse-lsp
RUN ./scripts/generate_guide_docs.sh
RUN ./tmp/fuse-target/release/fuse build --manifest-path docs
RUN mkdir -p docs/runnables/linux-x64 docs/downloads
RUN cp ./tmp/fuse-target/release/fuse docs/runnables/linux-x64/fuse
RUN cp ./tmp/fuse-target/release/fuse-lsp docs/runnables/linux-x64/fuse-lsp
RUN chmod 0755 docs/runnables/linux-x64/fuse docs/runnables/linux-x64/fuse-lsp
RUN bsdtar -C docs/runnables/linux-x64 -a -cf docs/downloads/fuse-pre-alpha-linux-x64.zip fuse fuse-lsp

FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates libsqlite3-0 \
  && rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/tmp/fuse-target/release/fuse /usr/local/bin/fuse
COPY --from=builder /workspace/docs /app/docs

EXPOSE 4080
ENV PORT=4080
ENV FUSE_HOST=0.0.0.0

CMD ["fuse", "run", "--manifest-path", "/app/docs"]

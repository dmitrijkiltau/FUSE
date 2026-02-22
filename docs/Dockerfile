# syntax=docker/dockerfile:1

FROM rust:slim-bookworm AS builder
WORKDIR /workspace

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates pkg-config libsqlite3-dev \
  && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY scripts/cargo_env.sh ./scripts/cargo_env.sh

RUN chmod +x scripts/cargo_env.sh
RUN ./scripts/cargo_env.sh cargo build -p fuse --release

COPY fls.md runtime.md scope.md ./
COPY scripts/generate_guide_docs.sh ./scripts/generate_guide_docs.sh
COPY docs ./docs

RUN chmod +x scripts/generate_guide_docs.sh
RUN ./scripts/generate_guide_docs.sh
RUN ./tmp/fuse-target/release/fuse build --manifest-path docs --aot --release

FROM debian:bookworm-slim AS runtime
WORKDIR /app/docs

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates libsqlite3-0 \
  && rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/docs /app/docs
COPY --from=builder /workspace/docs/.fuse/build/program.aot /app/docs/docs-aot

EXPOSE 4080
ENV PORT=4080
ENV FUSE_HOST=0.0.0.0
ENV FUSE_STATIC_DIR=/app/docs

CMD ["./docs-aot"]

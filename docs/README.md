# FUSE Docs Package

This directory contains the docs app served by the `docs` package.

## Bundled Runnables

Prebuilt binaries are generated during Docker image build (not committed).

- generated: `docs/runnables/linux-x64/fuse`
- generated: `docs/runnables/linux-x64/fuse-lsp`

Packaged ZIP for direct download:

- generated: `docs/downloads/fuse-pre-alpha-linux-x64.zip`

## Pre-alpha Download (ZIP)

If docs are running from Docker:

```bash
curl -LO http://localhost:4080/downloads/fuse-pre-alpha-linux-x64.zip
```

Then extract and verify:

```bash
unzip fuse-pre-alpha-linux-x64.zip
./fuse
```

## Run Docs Locally

From the repository root:

```bash
./scripts/generate_guide_docs.sh
./scripts/fuse run --manifest-path docs
```

Docs are served at <http://localhost:4080>.

## Run Docs with Docker

The Docker image builds `fuse`/`fuse-lsp` and creates ZIP/download artifacts during image build.
Run these commands from the repository root.

Build the image:

```bash
docker build -f docs/Dockerfile -t fuse-docs:pre-alpha .
```

Run:

```bash
docker run --rm -p 4080:4080 -e PORT=4080 -e FUSE_HOST=0.0.0.0 fuse-docs:pre-alpha
```

Or run with Compose:

```bash
docker compose -f docs/docker-compose.yml up --build
```

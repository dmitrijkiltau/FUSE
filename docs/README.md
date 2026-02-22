# FUSE Docs Package

This directory contains the docs app served by the `docs` package.

## Release Downloads

Downloads are published on GitHub Releases:

- https://github.com/dmitrijkiltau/FUSE/releases

## Run Docs Locally

From the repository root:

```bash
./scripts/generate_guide_docs.sh
./scripts/fuse run --manifest-path docs
```

Docs are served at <http://localhost:4080>.

## Run Docs with Docker

The Docker image builds and runs docs using the docs AOT binary.
It does not publish downloadable release artifacts.
Run these commands from the repository root.
The first `--build` run compiles Rust crates and can take a while; subsequent builds are much faster with Docker layer cache.

Build the image:

```bash
docker build -f docs/Dockerfile -t fuse-docs:0.4.0 .
```

Run:

```bash
docker run --rm -p 4080:4080 -e PORT=4080 -e FUSE_HOST=0.0.0.0 fuse-docs:0.4.0
```

Or run with Compose:

```bash
docker compose -f docs/docker-compose.yml up --build
```

After the image is built once, start faster without rebuilding:

```bash
docker compose -f docs/docker-compose.yml up
```

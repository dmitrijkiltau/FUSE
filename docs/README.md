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

The Docker image downloads the `fuse` CLI from GitHub Releases, builds docs with `fuse build --aot --release`, and runs the resulting AOT binary.
Only the binary, static site assets, and SVGs are copied into the runtime image.
A `HEALTHCHECK` pings `/api/health` every 30 s.
Guide docs regeneration is skipped in Docker because generated guides are committed.
It does not publish downloadable release artifacts.
Run these commands from the repository root.
You can override the downloaded CLI version with `--build-arg FUSE_RELEASE_TAG=vX.Y.Z`.

Build the image:

```bash
docker build -f docs/Dockerfile -t fuse-docs:0.5.0 .
```

Run:

```bash
docker run --rm -p 4080:4080 -e PORT=4080 -e FUSE_HOST=0.0.0.0 fuse-docs:0.5.0
```

Or run with Compose (from the repository root):

```bash
docker compose --project-directory . -f docs/docker-compose.yml up --build
```

After the image is built once, start faster without rebuilding:

```bash
docker compose --project-directory . -f docs/docker-compose.yml up
```

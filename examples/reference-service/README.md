# Reference service example

This is the canonical Fuse reference service package.
It includes registration/login auth, session-scoped CRUD routes, DB migrations, OpenAPI generation,
and SCSS asset compilation.

## Requirements

- Optional: `sass` (Dart Sass) on `PATH` for full SCSS compatibility.
- `./scripts/fuse` includes a local fallback SCSS compiler for this package's import-based styles.

## Run

```bash
./scripts/fuse run examples/reference-service
```

Open the UI:

```
http://localhost:3000/
```

Environment:

```
APP_PORT=3000
FUSE_DB_URL=sqlite://reference-service.db
```

## Migrate DB

```bash
./scripts/fuse migrate examples/reference-service
```

## Build

```bash
# Compile Fuse package and SCSS assets
./scripts/fuse build --manifest-path examples/reference-service

# Build deployable AOT artifact
./scripts/fuse build --manifest-path examples/reference-service --aot
```

SCSS pipeline:

- source: `assets/scss/style.scss` (+ partials in `assets/scss/`)
- output: `assets/css/style.css`

## OpenAPI

```bash
./scripts/fuse openapi --manifest-path examples/reference-service > /tmp/reference-service.openapi.json
```

## API

- `POST /api/auth/register`
- `POST /api/auth/login`
- `DELETE /api/auth/sessions/{token}`
- `GET /api/sessions/{token}/notes`
- `GET /api/sessions/{token}/notes/{id}`
- `POST /api/sessions/{token}/notes`
- `PUT /api/sessions/{token}/notes/{id}`
- `DELETE /api/sessions/{token}/notes/{id}` (idempotent)

The UI uses JSON requests and stores the returned opaque session token in browser localStorage.

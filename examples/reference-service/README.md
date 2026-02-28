# Reference service example

This is the canonical Fuse reference service package.
It includes registration/login auth, session-scoped CRUD routes, DB migrations, OpenAPI generation,
native CSS assets, and a server-rendered HTMX UI implemented in Fuse HTML DSL.

## Requirements

- No external frontend build tools are required.

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
# Compile Fuse package and static assets
./scripts/fuse build --manifest-path examples/reference-service

# Build deployable AOT artifact
./scripts/fuse build --manifest-path examples/reference-service --aot
```

CSS pipeline:

- entry: `assets/css/style.css`
- modules: `assets/css/tokens.css`, `assets/css/buttons.css`, `assets/css/forms.css`,
  `assets/css/dialog.css`, `assets/css/card.css`, `assets/css/layout.css`
- features: `@import`, custom properties (`--*`), and native CSS nesting

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
- `PUT /api/sessions/{token}/notes/{id}/visibility`
- `DELETE /api/sessions/{token}/notes/{id}` (idempotent)
- `POST /api/sessions/{token}/public/notes/{id}/likes` (idempotent; non-owner only)
- `GET /api/public/notes`
- `GET /api/public/notes/{id}`

Private session routes are owner-scoped: users only see and mutate their own notes.
Published notes are readable without authentication via the public routes.
Authenticated non-owners can leave likes on published notes.

## UI routes (HTMX + Html DSL)

- `GET /` (server-rendered app shell)
- `GET /sessions/{token}` (server-rendered shell for an existing token)
- `POST /ui/auth/register`
- `POST /ui/auth/login`
- `DELETE /ui/auth/logout`
- `POST /ui/notes`
- `PUT /ui/notes/{id}`
- `PUT /ui/notes/{id}/visibility`
- `DELETE /ui/notes/{id}`
- `POST /ui/public/notes/{id}/likes`
- `GET /ui/public/notes` (public feed fragment; viewer context derived from cookie)

The browser UI no longer depends on client-side note rendering.
Session state is persisted using an HTTP-only `sid` cookie (set by UI auth routes).
The package now declares a single HTTP service, so `fuse run` does not require `FUSE_SERVICE`.

UI module layout:

- `src/ui/pages/home.fuse`
- `src/ui/components/base.fuse`
- `src/ui/components/auth.fuse`
- `src/ui/components/create.fuse`
- `src/ui/components/notes.fuse`

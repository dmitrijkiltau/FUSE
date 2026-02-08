# Notes API example

This is a small Fuse example service with a static HTML UI.

## Run

```bash
./scripts/fuse run examples/notes-api
```

Open the UI:

```
http://localhost:3000/
```

Environment:

```
APP_PORT=3000
FUSE_DB_URL=sqlite://notes.db
```

## Migrate DB

```bash
./scripts/fuse migrate examples/notes-api
```

## API

- `GET /api/notes`
- `GET /api/notes/{id}`
- `POST /api/notes`
- `PUT /api/notes/{id}`
- `DELETE /api/notes/{id}` (idempotent)

The UI uses JSON requests; the service expects JSON bodies.

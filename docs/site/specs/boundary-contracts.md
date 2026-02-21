# Boundary Contracts and Error Mapping

Define request/response behavior in signatures, not ad hoc glue code.
Generated from annotated FUSE source.

_Generated from `docs/src/guides/boundary-contracts.fuse` by `scripts/generate_guide_docs.sh`._


## 1) Input type with refinements
```fuse
type NoteCreate:
  title: String(1..120)
  content: String(1..5000)

type Note:
  id: String
  title: String(1..120)
  content: String(1..5000)

```

## 2) Service signatures define HTTP contracts
- `body NoteCreate` means typed JSON decode + validation at the boundary
- each route declares its own error type (`Note!BadRequest`, `Note!NotFound`)
```fuse
import { BadRequest, NotFound } from std.Error

fn store_note(payload: NoteCreate) -> Note!BadRequest:
  if payload.title == "":
    return null ?! BadRequest(message="title is required")
  return Note(id="note-1", title=payload.title, content=payload.content)

service Notes at "/api":
  post "/notes" body NoteCreate -> Note!BadRequest:
    return store_note(body)

  get "/notes/{id: Id}" -> Note!NotFound:
    return null ?! NotFound(message="note not found")

```

## 3) Runtime behavior at a glance
- validation errors map to HTTP 400
- `std.Error.NotFound` maps to HTTP 404
- error JSON uses a stable `{"error": ...}` envelope

## 4) Why this model scales
Keep types/signatures authoritative and runtime behavior stays predictable.


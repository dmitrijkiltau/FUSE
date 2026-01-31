---
title: Use cases
description: Real-world scenarios where Fuse fits well.
---

Fuse is most useful when you want **strict data shapes**, **predictable config**, and
**clear error reporting** without a heavy framework.

## Internal JSON APIs

Small services that validate inputs, return structured errors, and evolve with
stable data contracts.

```
service Users at "/api":
  post "/users" body UserCreate -> User!BadRequest:
    return create_user(body)
```

Why it fits: typed request/response, refined constraints, consistent JSON errors.

## Config-driven workers

Background jobs with validated config and simple orchestration.

```
config Worker:
  interval: Int = env("INTERVAL") ?? 30

app "worker":
  print("tick")
```

Why it fits: predictable config layering and strict validation before work starts.

## ETL edge services

Ingest partner payloads, validate schemas, and normalize data.

```
service Ingest at "/ingest":
  post "/event" body PartnerEvent -> Ok!ValidationError:
    return Ok
```

Why it fits: structured decoding + refined types for early rejection.

## CLI automation

Safe, typed CLIs for internal automation (release tasks, migrations, user ops).

```
fn main(env: String, dry: Bool?) -> Result<String, Error>:
  return "ok"
```

Why it fits: argument → type binding + consistent error JSON.

## Gateway adapters

Thin layers that translate external API shapes into internal models.

```
service Gateway at "/v1":
  post "/notify" body ExternalPayload -> InternalAck:
    return transform(body)
```

Why it fits: explicit types on both sides with clear mapping points.

---
title: Use cases
description: Common patterns and workflows.
---

## Small JSON APIs

Fuse services model requests and responses as typed values.

```
service Users at "/api":
  post "/users" body UserCreate -> User:
    return create_user(body)
```

## CLI tools

Fuse apps can behave like CLIs by binding `fn main` params.

```
fn main(name: String, dry: Bool?) -> String:
  return "Hello, ${name}!"
```

## Config-driven workers

```
config Worker:
  interval: Int = env("INTERVAL") ?? 30

app "worker":
  print("tick")
```

## Prototyping

Fuse is designed for small, clear programs where data shapes and constraints
are explicit. Start with types and config, then add behavior.

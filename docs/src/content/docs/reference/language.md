---
title: Language reference
description: Core syntax and semantics.
---

## Declarations

- `type` for structs
- `enum` for tagged unions
- `fn` for functions
- `config` for config sections
- `app` for entry points
- `service` for HTTP services

## Types

Built-ins: `Int`, `Float`, `Bool`, `String`, `Id`, `Email`, `Bytes`.

Generics:

- `Option<T>` or `T?`
- `Result<T, E>` or `T!E`
- `List<T>`
- `Map<K, V>`

Refined types:

- `String(1..80)`
- `Int(0..100)`

## Expressions

- Literals: numbers, strings, `true`, `false`, `null`
- Calls: `print("hi")`
- Interpolation: `"Hello, ${name}"`
- Pattern matching: `match` with `case`

## Pattern matching

```
match value:
  case Ok(x):
    return x
  case Err(e):
    return e
```

## Services

```
service Users at "/api":
  get "/users/{id: Id}" -> User!NotFound:
    return get_user(id)

  post "/users" body User -> User:
    return body
```

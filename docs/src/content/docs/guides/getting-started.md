---
title: Language tour
description: A quick tour of Fuse syntax and concepts.
---

## Types

Fuse uses structs and enums for data modeling.

```
type User:
  id: Id
  name: String(1..80)
  email: Email

enum Status:
  Ok
  Error(message: String)
```

## Functions

```
fn greeting(name: String) -> String:
  return "Hello, ${name}!"
```

## Optional and Result

- `T?` is optional (`null` represents None)
- `T!E` is a Result with error type `E`

```
fn find(id: Id) -> User?:
  return null

fn get(id: Id) -> User!NotFound:
  let user = find(id) ?! NotFound(message="not found")
  return user
```

## Config

Configs resolve from env, then config files, then defaults.

```
config App:
  port: Int = env("PORT") ?? 3000
```

## Services

```
service Users at "/api":
  get "/users/{id: Id}" -> User!NotFound:
    return get(id)

  post "/users" body User -> User:
    return body
```

## Apps

```
app "users":
  serve(App.port)
```

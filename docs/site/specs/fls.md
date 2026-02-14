# FUSE Syntax and Types

This page is a practical reference for writing valid FUSE code.

---

## Tokens and blocks

- identifiers: `[A-Za-z_][A-Za-z0-9_]*`
- strings are double-quoted and support `${expr}` interpolation
- comments: `#` line comments, `##` doc comments
- blocks are introduced with `:` and controlled by indentation
- tabs are illegal; use spaces

Example:

```fuse
fn greet(name: String) -> String:
  if name == "":
    return "hi"
  return "hi ${name}"
```

See also: [Expressions and control flow](#expressions-and-control-flow), [Language Guide](fuse.md).

---

## Expressions and control flow

FUSE supports:

- arithmetic/comparison/logical operators
- null coalescing (`??`)
- optional access (`?.`, `?[idx]`)
- bang-chain (`?!`) for option/result conversion
- `if`, `match`, `for`, `while`, `break`, `continue`
- `spawn`, `await`, and `box`

Example `match`:

```fuse
enum Status:
  Pending
  Done(String)

fn describe(s: Status) -> String:
  match s:
    Pending -> "waiting"
    Done(msg) -> msg
```

See also: [Runtime Guide](runtime.md), [Error handling and status mapping](runtime.md#error-handling-and-status-mapping).

---

## Types and declarations

Core declarations:

- `type` for structs
- `enum` for tagged variants
- `fn` for functions
- `config` for typed settings
- `service` for HTTP endpoints
- `app` for executable entry blocks

Type features:

- optionals: `T?`
- results: `T!` and `T!E`
- generics: `List<T>`, `Map<K,V>`, `Result<T,E>`
- refinements: `String(1..80)`, `Int(0..130)`, `Float(0.0..1.0)`
- derivations: `type Public = User without password`

See also: [Boundary behavior](runtime.md#boundary-behavior), [Scope and roadmap](scope.md).

---

## Imports and modules

Import forms:

```fuse
import Foo
import Utils from "./utils"
import {A, B} from "./lib"
import Auth from "dep:Auth/lib"
```

Guidelines:

- module imports are qualified (`Foo.value`, `Foo.Type`)
- named imports bring selected names into local scope
- type references may be module-qualified (`Foo.User`)

See also: [Language Guide](fuse.md), [Runtime Guide](runtime.md).

---

## Services and routes

Route declarations are typed at the signature level:

```fuse
service Api at "/api":
  get "/users/{id: Id}" -> User:
    return load_user(id)

  post "/users" body UserCreate -> User:
    return create_user(body)
```

Use `Html` return types for server-rendered fragments when needed.

See also: [HTTP Behavior](runtime.md#http-behavior), [Language Guide](fuse.md).

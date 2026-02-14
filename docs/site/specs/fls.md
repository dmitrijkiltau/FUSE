# Language Tour

This guide teaches the FUSE language by example.

---

## 1) Declarations and blocks

FUSE uses indentation-based blocks and explicit declarations:

- `fn` for functions
- `type` for structs
- `enum` for variants
- `config` for typed settings
- `service` for HTTP endpoints
- `app` for runnable entry blocks

```fuse
fn greet(name: String) -> String:
  if name == "":
    return "hi"
  return "hi ${name}"
```

---

## 2) Core expressions

Common expression features:

- null coalescing: `x ?? fallback`
- optional access: `obj?.field`, `arr?[idx]`
- error conversion: `expr ?! err`
- control flow: `if`, `match`, `for`, `while`

```fuse
enum Status:
  Pending
  Done(String)

fn describe(s: Status) -> String:
  match s:
    Pending -> "waiting"
    Done(msg) -> msg
```

If you want the runtime meaning of these error and control-flow forms in HTTP services, jump to [Build + Operate](runtime.md#2-error-handling-and-status-mapping).

---

## 3) Type patterns you will use every day

- optionals: `T?`
- results: `T!` and `T!E`
- collections: `List<T>`, `Map<K,V>`
- refined primitives: `String(1..80)`, `Int(0..130)`
- derivations: `type PublicUser = User without password`

```fuse
type User:
  email: Email
  name: String(1..80)
  age: Int(0..130) = 18
```

---

## 4) Modules and imports

```fuse
import Foo
import Utils from "./utils"
import {A, B} from "./lib"
import Auth from "dep:Auth/lib"
```

Guidelines:

- module imports are qualified (`Foo.value`, `Foo.Type`)
- named imports bring selected names into local scope
- types can be module-qualified (`Foo.User`)

---

## 5) Service signatures

Routes are typed at declaration time:

```fuse
service Api at "/api":
  get "/users/{id: Id}" -> User:
    return load_user(id)

  post "/users" body UserCreate -> User:
    return create_user(body)
```

`body` introduces typed JSON input, and return types define output contracts.

Next step: build this into a runnable package with [Start Here](fuse.md), then verify runtime behavior in [Build + Operate](runtime.md).

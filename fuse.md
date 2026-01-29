# **FUSE**

*Write intent. Get software.*

FUSE is a small, strict, “default-sane” language that assumes you want to build real apps (CLI, HTTP APIs, jobs, small services) without spending 40% of your life wiring configs, serializers, validators, routers, and dependency injection.

## The core vibe

### 1) Everything is a module, modules are cheap

No folders-as-religion. Every file is a module. Imports are obvious.

### 2) Strong types, zero ceremony

Types exist so your code doesn’t lie. But you shouldn’t have to negotiate with the compiler like it’s a bank clerk.

### 3) “Boilerplate” is treated as a bug

Common patterns are built-in:

* config loading + env overrides
* logging
* JSON (de)serialization
* validation
* HTTP routing
* migrations (basic)
* test runner
* docs generation (OpenAPI-ish)
* CLI flags parsing

You write **intent**, not glue.

## Syntax: aggressively readable

* Indentation-based blocks (yes, like Python, but with a spine).
* No semicolons.
* `let` for immutable, `var` for mutable.
* Functions are `fn`.
* Structs are `type`.
* Enums are `enum`.

### Hello World (CLI)

```fuse
app "hello":
  fn main(name: String = "world"):
    print "Hello, {name}!"
```

You didn’t define args parsing. FUSE did. Default value becomes optional flag: `--name`.

## Data model: types that *do things*

### Types include validation rules inline

```fuse
type User:
  id: Id
  email: Email
  name: String(1..80)
  age: Int(0..130) = 18
```

* `Email` is a built-in refined type.
* `String(1..80)` is a constrained string.
* Default value works everywhere: constructors, decoding, forms, etc.

### Auto-generated things

From that type, FUSE can generate:

* JSON schema
* runtime validator
* DB mapping hints
* API docs
* example payloads for tests

## Functions: small and explicit

```fuse
fn greet(user: User) -> String:
  "Hi {user.name}"
```

Expression-last returns implicitly, but you can `return` when you feel dramatic.

## Errors: not exceptions, not misery

FUSE uses a `Result<T, E>` style, but with sugar that doesn’t make you cry.

```fuse
fn parseAge(s: String) -> Int?:
  Int.try(s)

fn loadUser(id: Id) -> User!:
  db.users.get(id) ?! NotFound("User {id}")
```

* `T?` is optional.
* `T!` means “may fail” (a Result).
* `?!` means “if empty/fail, throw this typed error” (still not a runtime exception, it’s a propagated error).

## Concurrency: boring and safe

* `spawn` gives you a task handle.
* `await` waits.
* Shared mutable state requires `box` (explicit).

```fuse
var counter = box 0

spawn:
  counter += 1

await all
print counter
```

## HTTP: you describe endpoints, FUSE builds the plumbing

```fuse
service Users at "/api":
  get "/users/{id: Id}" -> User:
    db.users.get(id) ?! NotFound()

  post "/users" body UserCreate -> User:
    let user = db.users.insert(body)
    return user
```

You did not:

* set up a router
* write request parsing
* write validation
* write JSON encoding
* write error mapping

FUSE did, and also produces API docs automatically.

### DTOs are derived automatically

```fuse
type UserCreate = User without id
```

Yes. Because obviously.

## Database: “good defaults” not a full ORM nightmare

FUSE has a built-in `db` interface with:

* migrations
* typed queries
* transaction blocks

```fuse
migration 001 "create users":
  table users:
    id Id primary
    email Email unique
    name String
    age Int

fn adults() -> List<User>:
  db.users.where(age >= 18).all()
```

## Tests: first-class, zero setup

```fuse
test "age defaults to 18":
  let u = User(email="a@b.de", name="Dima")
  assert u.age == 18
```

## Packages: no build tool soap opera

* `fuse.toml` for deps
* one command: `fuse run`, `fuse test`, `fuse build`
* reproducible lockfile

## The “boilerplate killer” rules

FUSE automatically provides these unless you opt out:

1. **JSON codec** for every `type` and `enum`
2. **Validator** from constraints
3. **CLI** argument parsing from `main` signature
4. **HTTP** routing + docs from `service` blocks
5. **Config** binding from `config` blocks
6. **Logging** with structured fields
7. **Error -> HTTP mapping** with sane defaults

## One bigger example: tiny user API + config + logging

```fuse
config App:
  port: Int = env("PORT") ?? 3000
  dbUrl: String = env("DB_URL") ?? "sqlite://app.db"

type User:
  id: Id
  email: Email
  name: String(1..80)
  age: Int(0..130) = 18

service Users at "/api":
  get "/health" -> Map<String, String>:
    {"status": "ok"}

  post "/users" body User without id -> User:
    log.info "Creating user", email=body.email
    db.users.insert(body)

app "users":
  fn main():
    db.connect(App.dbUrl)
    serve port=App.port
```

## “Okay but what’s novel?”

Not the syntax. The novelty is the **contract**:
You write *types + intent*, the language runtime/tooling guarantees:

* validation always happens
* errors are typed and mapped consistently
* docs are always generated
* serialization is never hand-written
* config is never manual
* CLI and HTTP share the same parameter model

Basically: “framework behavior” is language-level, so it’s consistent and not 12 competing libraries duct-taped together.

## Scope

> [scope.md](scope.md)

## Formal Language Specification

> [fls.md](fls.md)
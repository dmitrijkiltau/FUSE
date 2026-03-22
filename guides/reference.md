# FUSE Language Reference

FUSE is a small, strict language for CLI tools and HTTP services. Every runtime
surface, including config loading, JSON binding, validation, and HTTP routing, is built in and
type-checked. This document is the primary reference for developers and AI agents
learning the language. For normative grammar and runtime semantics see `spec/fls.md`
and `spec/runtime.md`.

---

## Declarations

A FUSE file is a flat sequence of top-level declarations. Order within the file does
not matter for resolution.

```fuse
requires db          # capability gate (compile-time)
requires network

import Auth from "./auth"             # module import
import {verify} from "./crypto_util"  # named import
import Docs from "./README.md"        # asset import (String)
import Seeds from "./seed.json"       # asset import (decoded value)

config App:                           # config block
  port: Int = env_int("PORT") ?? 3000
  db_url: String = env("DATABASE_URL") ?? "sqlite://app.db"

type User:                            # struct declaration
  id: Id
  email: Email
  name: String(1..80)

enum Role:                            # enum declaration
  Admin
  Member(permissions: List<String>)

fn greet(name: String) -> String:    # function
  return "Hello, ${name}!"

service Users at "/api":             # HTTP service
  get "/users/{id: Id}" -> User!NotFound:
    ...

app "api":                            # entry point
  serve(App.port)

migration "create_users":            # DB migration
  db.exec("create table if not exists users (id text primary key, name text)")

test "greet returns greeting":        # test block
  assert(greet("world") == "Hello, world!")

component Card:                       # HTML component
  return div(class="card"):
    children
```

---

## Types

### Primitives

```fuse
let s: String = "hello"
let n: Int    = 42
let f: Float  = 3.14
let b: Bool   = true
let id: Id    = "usr_01"     # non-empty string
let em: Email = "a@b.com"   # simple local@domain check
```

### Strings

```fuse
let greeting = "Hello, ${name}!"          # interpolation in standard strings

let query = """
  select *
  from users
  where id = ?
"""                                        # multiline string; interpolation works here too

# Comments use #. Doc comments (##) attach to the next declaration.
## This doc comment belongs to the function below.
fn example(): ...
```

### Structs

```fuse
type Point:
  x: Float
  y: Float
  label: String = "origin"    # default value

# Construct with named fields:
let p = Point(x = 1.0, y = 2.0)
let p2 = Point(x = 0.0, y = 0.0, label = "zero")

# Field access:
print(p.label)

# Derived type that removes fields from a base type:
type PublicUser = User without password, secret
```

### Enums

```fuse
enum Status:
  Active
  Suspended(reason: String)
  Banned(reason: String, since: Int)

# Match on an enum:
match user.status:
  Active -> serve_request()
  Suspended(reason = r):
    return Err(std.Error.Forbidden(message = "suspended: ${r}"))
  Banned(reason = r, since = _):
    return Err(std.Error.Forbidden(message = "banned: ${r}"))
```

Enums encode to JSON as a tagged object:

```json
{ "type": "Suspended", "data": "terms violation" }
```

No payload omits `data`. Multiple payloads use an array for `data`.

### Options

```fuse
fn find(id: Id) -> User?:     # User? is Option<User>
  ...

let u = find("x")
let name = u?.name            # optional field access; yields null if u is null
let display = u?.name ?? "anonymous"   # null-coalescing

match u:
  None -> print("not found")
  Some(user) -> print(user.name)
```

### Results

```fuse
fn create(email: Email) -> User!ValidationError:   # T!E is Result<T, E>
  ...

# T! without an error domain is a compile-time error. Always name the error type.

let result = create("bad")
match result:
  Ok(user) -> print(user.id)
  Err(e)   -> print(e.message)
```

### Refinements

Refinements attach constraints to primitive types and are validated at boundaries
(struct construction, JSON decode, config loading, route params).

```fuse
type Slug:
  value: String(1..80, regex("^[a-z0-9-]+$"))

type Measurement:
  weight_kg: Float(0.0..500.0)
  age: Int(0..130)

# Custom predicate:
fn is_even(n: Int) -> Bool: return n % 2 == 0
type EvenInt:
  n: Int(predicate(is_even))
```

Constraints apply left-to-right. `regex` is valid on `String`, `Id`, and `Email`.

### Type inference

```fuse
let x = 42           # inferred Int
let s = "hello"      # inferred String
var items = [1, 2]   # inferred List<Int>

# Parameter types are always required. Return types are optional (inferred when omitted).
fn double(n: Int) -> Int:
  return n * 2
```

---

## Variables and Control Flow

### let and var

```fuse
let x = 10        # immutable binding
var count = 0     # mutable binding
count = count + 1
```

### if / else

```fuse
if count > 10:
  print("high")
else if count > 5:
  print("medium")
else:
  print("low")

# Inline form:
if flag: x = 1
```

### match

```fuse
match value:
  0 -> print("zero")
  1 -> print("one")
  _ -> print("other")

# Block form (multiple statements):
match result:
  Ok(data):
    process(data)
    log("done")
  Err(e):
    log("error", e.message)

# Struct pattern with field-name bindings:
match point:
  Point(x = 0.0, y = 0.0) -> print("origin")
  Point(x = px, y = py)   -> print("at ${px}, ${py}")
```

### for and while

```fuse
for item in items:
  print(item)

for value in map:     # iterates values only; keys are not exposed in for bodies
  print(value)

var i = 0
while i < 10:
  i = i + 1

# break / continue work as expected:
for x in list:
  if x == 0: continue
  if x < 0: break
  process(x)
```

### Ranges

```fuse
for i in 1..5:       # inclusive: [1, 2, 3, 4, 5]
  print(i)

let digits = 0..9    # List<Int>
```

Float ranges step by `1.0`. `a > b` is a runtime error.

### List and Map indexing

```fuse
let first = items[0]         # runtime error if out of bounds
let val   = map["key"]       # null if key is missing
items[0]  = "updated"        # bounds-checked assignment
map["k"]  = "v"              # insert or overwrite

let safe = items?[0]         # null if out of bounds (optional index)
```

---

## Functions

```fuse
fn add(a: Int, b: Int) -> Int:
  return a + b

# Default parameter values:
fn greet(name: String = "world") -> String:
  return "Hello, ${name}!"

greet()            # "Hello, world!"
greet("FUSE")      # "Hello, FUSE!"

# Named arguments at call sites:
fn range_check(value: Int, min: Int = 0, max: Int = 100) -> Bool:
  return value >= min and value <= max

range_check(value = 50, max = 80)
```

Functions are module-scoped. Unqualified calls resolve in the current module first,
then named imports.

---

## Modules and Imports

```fuse
# Module import with qualified access:
import Auth from "./auth"
let token = Auth.create_token(user.id)

# Named import that brings specific names into scope:
import {hash, verify} from "./crypto_util"
let h = hash("sha256", data)

# Alias import:
import Helpers as H from "./helpers"

# Package root import (from fuse.toml directory):
import Config from "root:config/defaults"

# Dependency import (declared in fuse.toml [dependencies]):
import {validate} from "dep:Validator/lib"

# Asset imports:
import Policy from "./POLICY.md"    # Policy is a String
import Seeds  from "./seed.json"    # Seeds is a decoded runtime value

print(Policy)
print(json.encode(Seeds))
```

Asset imports are values, not modules. They do not create a namespace or expose named exports.

### Capabilities

Capabilities gate access to runtime builtins at compile time.

```fuse
requires db
requires network
requires time
requires crypto
```

| Capability | Gates access to |
|---|---|
| `db` | `db.*`, `transaction:` |
| `network` | `serve(...)`, `http.*` |
| `time` | `time.*` |
| `crypto` | `crypto.*` |

Calling an imported function that requires a capability you haven't declared is a
compile-time error. Capabilities do not leak across module boundaries silently.

---

## Config

```fuse
config App:
  port:    Int    = env_int("PORT") ?? 3000
  host:    String = env("HOST") ?? "0.0.0.0"
  db_url:  String = env("DATABASE_URL") ?? "sqlite://app.db"
  debug:   Bool   = false

app "server":
  serve(App.port)
```

Config values resolve in this order:
1. Environment variable (e.g. `APP_PORT` for `App.port`)
2. Config file (`config.toml` by default, override with `FUSE_CONFIG`)
3. Default expression in the `config` block

The CLI loads `.env` from the package directory and injects missing variables before
resolution. Existing env vars are never overridden by `.env`.

Env var naming: `App.dbUrl` → `APP_DB_URL` (camelCase splits to `SNAKE_CASE`).

Config values support scalars (`Int`, `Float`, `Bool`, `String`, `Id`, `Email`,
`Bytes`) and `Option<T>` directly. `List`, `Map`, structs, and enums are accepted as
JSON text.

---

## Services and HTTP

```fuse
requires network

config App:
  port: Int = env_int("PORT") ?? 3000

type UserCreate:
  email: Email
  name:  String(1..80)

type NotFound:
  message: String

service Users at "/api":
  get "/users" -> List<User>:
    return db.from("users").all<User>()

  get "/users/{id: Id}" -> User!NotFound:
    let row = db.from("users").where("id", "=", id).one<User>()
    return row ?! NotFound(message = "user ${id} not found")

  post "/users" body UserCreate -> User!std.Error.Validation:
    db.from("users").insert(body).exec()
    return db.from("users").where("email", "=", body.email).one<User>() ?! ...

app "api":
  serve(App.port)
```

Route path params use `{name: Type}`. Supported types for params: scalars and
`Option<T>` of scalars. Refinement constraints apply at parse time.

The `body` keyword binds the JSON request body to `body` in the handler. Unknown
fields in the body are a validation error.

### Reading request context

```fuse
service Api at "/":
  get "/profile" -> Profile:
    let token  = request.cookie("session") ?! std.Error.Unauthorized(message = "no session")
    let req_id = request.header("x-request-id")
    # ... decode token, load profile
```

### Setting response headers and cookies

```fuse
  post "/login" body Credentials -> Session!AuthError:
    let session = Auth.create(body) ?!
    response.cookie("session", session.token)
    response.header("x-custom", "value")
    return session

  post "/logout" -> Map<String, String>:
    response.delete_cookie("session")
    return {"status": "ok"}
```

### Error → HTTP status mapping

Return a standard error type to get automatic status codes:

| Type | Status |
|---|---|
| `std.Error.Validation` | 400 |
| `std.Error.BadRequest` | 400 |
| `std.Error.Unauthorized` | 401 |
| `std.Error.Forbidden` | 403 |
| `std.Error.NotFound` | 404 |
| `std.Error.Conflict` | 409 |
| `std.Error` with `status: Int` | that status |
| anything else | 500 |

Error JSON shape:

```json
{ "error": { "code": "not_found", "message": "user not found" } }
```

`std.Error.Validation` adds a `fields` array:

```json
{ "error": { "code": "validation_error", "message": "...",
             "fields": [{ "path": "email", "code": "invalid_value", "message": "..." }] } }
```

### OpenAPI

Fuse can generate an OpenAPI 3.0 document for declared `service` routes.

Emit the document on stdout:

```bash
fuse openapi src/main.fuse
fuse openapi --manifest-path examples/reference-service
```

Write the document automatically during builds:

```toml
[build]
openapi = "build/openapi.json"
```

`fuse build` resolves `[build].openapi` relative to the package directory unless the path is
absolute, creates parent directories when needed, and writes the generated JSON after a
successful build.

Serve the built-in docs UI from the runtime:

```toml
[serve]
openapi_ui = true
openapi_path = "/docs"
```

Rules:
- `openapi_ui` defaults to `true` under `fuse dev` and `false` under `fuse run`.
- `openapi_path` defaults to `/docs` and names the HTML docs route.
- The raw JSON document is served at `<openapi_path>/openapi.json`.
- The runtime only serves the built-in docs UI for `GET` requests.

---

## HTTP Client

```fuse
requires network

fn fetch_user(id: String) -> Map<String, String>!http.error:
  let resp = http.get(
    "https://api.example.com/users/${id}",
    headers = {"Authorization": "Bearer ${token}"}
  ) ?!
  return json.decode(resp.body)

fn post_event(url: String, payload: String) -> Bool!http.error:
  let resp = http.post(url, body = payload, timeout_ms = 5000) ?!
  return resp.status == 200
```

API:
- `http.get(url, headers?, timeout_ms?) -> http.response!http.error`
- `http.post(url, body, headers?, timeout_ms?) -> http.response!http.error`
- `http.request(method, url, body?, headers?, timeout_ms?) -> http.response!http.error`

`http.response` fields: `method`, `url`, `status`, `headers`, `body`
`http.error` fields: `code`, `message`, `method`, `url`, `status?`, `headers`, `body?`

Error codes: `http_status` (non-2xx), `tls_error`, `timeout`, `network_error`,
`invalid_url`, `invalid_request`, `invalid_response`, `unsupported_scheme`.

`timeout_ms` defaults to `30000`. `0` disables the timeout. Redirects are manual
(`3xx` surfaces as `http_status`).

---

## Database

```fuse
requires db

config App:
  db_url: String = env("FUSE_DB_URL") ?? "sqlite://app.db"

migration "create_users":
  db.exec("""
    create table if not exists users (
      id   text primary key,
      name text not null,
      role text not null default 'member'
    )
  """)

type User:
  id:   Id
  name: String
  role: String

fn all_users() -> List<User>:
  return db.from("users").select(["id", "name", "role"]).all<User>()

fn find_user(id: Id) -> User?:
  return db.from("users").where("id", "=", id).select(["id", "name", "role"]).one<User>()

fn create_user(id: Id, name: String) -> Bool:
  db.from("users").insert(User(id = id, name = name, role = "member")).exec()
  return true
```

### Raw SQL

```fuse
# Execute (no return):
db.exec("delete from users where role = ?", ["banned"])

# All rows as List<Map<String, Value>>:
let rows = db.query("select id, name from users where active = ?", [true])

# First row or null:
let row = db.one("select * from users where id = ?", [id])
```

### Query builder

```fuse
# Typed reads; columns must match the target type fields:
let users = db.from("users")
  .select(["id", "name", "role"])
  .where("role", "=", "admin")
  .order_by("name", "asc")
  .limit(20)
  .all<User>()

# Write operations:
db.from("users").insert(user).exec()
db.from("users").upsert(user).exec()
db.from("users").where("id", "=", id).update("name", new_name).exec()
db.from("users").where("id", "=", id).delete().exec()

let n = db.from("users").where("role", "=", "admin").count()
```

`where` operators: `=`, `!=`, `<`, `<=`, `>`, `>=`, `like`, `in`.
`in` expects a non-empty list and expands to `IN (?, ?, ...)`.

### Transactions

```fuse
# Modules using transaction: must have requires db and no other capability.
requires db

fn transfer(from_id: Id, to_id: Id, amount: Int):
  transaction:
    db.from("accounts").where("id", "=", from_id).update("balance", balance - amount).exec()
    db.from("accounts").where("id", "=", to_id).update("balance", balance + amount).exec()
  # commits on success, rolls back if the block fails
```

Inside `transaction:`: no `spawn`, no `await`, no early `return`, no `break`/`continue`.

### Migrations

```fuse
migration "001_create_users":
  db.exec("create table if not exists users (id text primary key, name text)")

migration "002_add_role":
  db.exec("alter table users add column role text not null default 'member'")
```

Run pending migrations:

```bash
fuse migrate src/main.fuse
```

Migrations run in ascending name order. Applied migrations are tracked in
`__fuse_migrations(package, name)`.

---

## Error Handling

### Bang-chain propagation with ?!

```fuse
# Propagate Result errors up the call stack:
fn get_user(id: Id) -> User!NotFound:
  let row = db.from("users").where("id", "=", id).one<User>()
  return row ?! NotFound(message = "not found")

# Option ?! requires an explicit error value:
let token = request.cookie("session") ?! std.Error.Unauthorized(message = "no session")

# Result ?! without a value propagates the existing error:
let data = fetch_remote() ?!

# Chain multiple error domains on the return type:
fn create(input: Input) -> Output!ValidationError!DbError: ...
```

### Explicit match

```fuse
match db.from("users").where("id", "=", id).one<User>():
  None    -> return Err(NotFound(message = "not found"))
  Some(u) -> return Ok(u)

match http.get(url):
  Ok(resp)  -> return json.decode(resp.body)
  Err(e)    -> log("error", "fetch failed", e.code)
```

### Error types

Declare your own error types as regular structs:

```fuse
type NotFound:
  message: String

type AuthError:
  code:    String
  message: String
```

Use `std.Error.*` for automatic HTTP status mapping (see Services section).

---

## Concurrency

```fuse
fn fetch_all(ids: List<Id>) -> List<User>:
  # Spawn parallel tasks:
  let t1 = spawn:
    load_from_db(ids[0])
  let t2 = spawn:
    load_from_db(ids[1])

  # Await results. Both must be awaited before scope exit:
  let u1 = await t1
  let u2 = await t2
  return [u1, u2]
```

Rules enforced at compile time:
- Detached task expressions are rejected (must assign to a binding).
- A spawned task binding must be `await`ed before leaving its scope.
- Spawned task bindings cannot be reassigned before `await`.

Inside `spawn:` blocks, the following are rejected: `box` access, `db.*`, `serve`,
`print`, `log`, `env*`, `asset`, `svg.inline`, and mutation of captured outer bindings.
Keep side effects on the parent path; use `spawn` for pure compute.

### Shared mutable state

```fuse
let counter = box 0       # shared mutable cell

# Update from multiple call sites:
counter = counter + 1

# spawn blocks cannot capture or use boxed state.
```

---

## HTML DSL

```fuse
requires network

component Layout:
  return html():
    head():
      meta(charset="utf-8")
      title(): "My App"
    body(class="app"):
      children          # slot for child content

component Button:
  let label = attrs["label"] ?? "Click"
  return button(attrs class="btn"):
    label

fn home_page() -> Html:
  return Layout():
    h1(): "Welcome"
    Button(label="Get started")
    div(class="list"):
      for user in users:
        div(class="item"): user.name
```

### Component rules

- `component Name:` body must return `Html`.
- Implicit params: `attrs: Map<String, String>`, `children: List<Html>`.
- Use `attrs` as pass-through presentation attributes on the outer boundary element.
- Use `children` as the content slot for already-rendered `Html`.

### Attribute shorthand

```fuse
div(class="hero" id="main")         # space-separated, no commas
button(aria_label="Close" type="button")   # _ → - for aria-* and data-* attrs
```

`aria-*` attribute names and values are validated at compile time. Unknown `aria-*`
names, wrong bool values (`aria-hidden` only accepts `"true"` / `"false"`), and
`aria-role` (use `role` instead) are compile-time errors.

### HTML builtins

`html.text(s)`, `html.raw(s)`, `html.node(tag, attrs, children)`, `html.render(h) -> String`
`svg.inline(path: String) -> Html` (requires `FUSE_SVG_DIR` or package-relative path)

Bare string literals in HTML blocks lower to `html.text(...)` automatically.

---

## Testing

```fuse
test "add works":
  assert(1 + 1 == 2)

test "user validation rejects short names":
  let result = validate_name("")
  match result:
    Err(_) -> assert(true)
    Ok(_)  -> assert(false, "expected error")

test "db round-trip":
  db.exec("delete from users")
  create_user("u1", "Alice")
  let u = find_user("u1")
  assert(u?.name == "Alice")
```

```bash
fuse test src/main.fuse
fuse test --filter "validation" src/main.fuse   # substring match, case-sensitive
```

Tests run in ascending name order via the AST interpreter.

---

## Logging

```fuse
log("server started")                        # INFO
log("warn", "high memory usage")             # WARN
log("error", "db connection failed")         # ERROR
log("debug", "request received", request)    # structured JSON (2+ args after level)
```

Output format: `[LEVEL] message` on stderr. ANSI color respects `FUSE_COLOR` / `NO_COLOR`.

`FUSE_LOG` sets the minimum level (default `info`). Levels: `trace`, `debug`, `info`,
`warn`, `error`.

Structured log output (when a data argument is present):

```json
{"level":"debug","message":"request received","data":{...}}
```

---

## CLI Apps

```fuse
fn main(name: String = "world", count: Int = 1):
  for _ in 1..count:
    print("Hello, ${name}!")

app "hello":
  main()
```

```bash
fuse run main.fuse -- --name=FUSE --count=3
```

CLI binding rules:
- Flags only, no positional args.
- `--flag value` and `--flag=value` both work.
- `--flag` sets `Bool` to `true`; `--no-flag` sets it to `false`.
- Unknown flags are validation errors (exit code 2).
- When CLI args are present, `app` is bypassed and `fn main` is called directly.

---

## Assets

```fuse
import Policy from "./POLICY.md"     # immutable String containing the exact UTF-8 contents
import Seeds  from "./seed.json"     # decoded runtime value (like json.decode result)

fn show_policy() -> Html:
  return pre(): Policy

fn seed_db():
  for row in Seeds:
    db.from("users").insert(row).exec()
```

Asset imports are values, not modules. Only `import Name from "path.ext"` is
supported for asset files (no named or aliased asset imports).

---

## Builtins Reference

### I/O

| Builtin | Signature | Description |
|---|---|---|
| `print` | `(value) -> ()` | Print stringified value to stdout |
| `input` | `(prompt: String = "") -> String` | Read one line from stdin (trims trailing newline) |
| `log` | `(level?, message, data?) -> ()` | Write to stderr (see Logging) |

### Environment

| Builtin | Returns | Description |
|---|---|---|
| `env(name)` | `String?` | Env var value or `null` |
| `env_int(name)` | `Int?` | Parsed env var or `null`; fatal if set but not parseable |
| `env_float(name)` | `Float?` | Parsed env var or `null`; fatal if set but not parseable |
| `env_bool(name)` | `Bool?` | Parsed env var or `null`; fatal if set but not parseable |

### JSON

| Builtin | Signature | Description |
|---|---|---|
| `json.encode` | `(value) -> String` | Serialize any value to JSON |
| `json.decode` | `(text: String) -> Value` | Parse JSON string to runtime value |

### Time (`requires time`)

| Builtin | Signature | Description |
|---|---|---|
| `time.now` | `() -> Int` | Unix epoch milliseconds |
| `time.sleep` | `(ms: Int)` | Block for ms milliseconds |
| `time.format` | `(epoch: Int, fmt: String) -> String` | Format epoch ms (UTC) |
| `time.parse` | `(text: String, fmt: String) -> Int!Error` | Parse to epoch ms |

### Crypto (`requires crypto`)

| Builtin | Signature | Description |
|---|---|---|
| `crypto.hash` | `(algo: String, data: Bytes) -> Bytes` | `sha256` / `sha512` |
| `crypto.hmac` | `(algo: String, key: Bytes, data: Bytes) -> Bytes` | HMAC |
| `crypto.random_bytes` | `(n: Int) -> Bytes` | Cryptographically secure random |
| `crypto.constant_time_eq` | `(a: Bytes, b: Bytes) -> Bool` | Timing-safe compare |

### Database (`requires db`)

| Builtin | Signature | Description |
|---|---|---|
| `db.exec` | `(sql, params?)` | Execute SQL, no return value |
| `db.query` | `(sql, params?)` | `List<Map<String, Value>>` |
| `db.one` | `(sql, params?)` | `Map<String, Value>?` |
| `db.from` | `(table: String) -> Query` | Start a query builder chain |
| `transaction:` | block | BEGIN/COMMIT/ROLLBACK scope |
| `assert` | `(cond: Bool, message?: String)` | Runtime error when `cond` is false |

### HTTP server (`requires network`)

| Builtin | Description |
|---|---|
| `serve(port: Int)` | Start HTTP server on `FUSE_HOST:port` |
| `request.header(name)` | Inbound header value (case-insensitive), `String?` |
| `request.cookie(name)` | Inbound cookie value, `String?` |
| `response.header(name, value)` | Append response header |
| `response.cookie(name, value)` | Append `Set-Cookie` |
| `response.delete_cookie(name)` | Emit cookie expiration header |

### HTTP client (`requires network`)

| Builtin | Signature |
|---|---|
| `http.get` | `(url, headers?, timeout_ms?) -> http.response!http.error` |
| `http.post` | `(url, body, headers?, timeout_ms?) -> http.response!http.error` |
| `http.request` | `(method, url, body?, headers?, timeout_ms?) -> http.response!http.error` |

### Assets and HTML

| Builtin | Description |
|---|---|
| `asset(path: String) -> String` | Resolve hashed public URL (requires `FUSE_ASSET_MAP`) |
| `svg.inline(path: String) -> Html` | Inline SVG as `Html` |
| `html.text(s: String) -> Html` | Text node |
| `html.raw(s: String) -> Html` | Unescaped HTML node |
| `html.render(h: Html) -> String` | Render `Html` to string |

---

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `FUSE_DB_URL` | `unset` | Database URL (`sqlite://path`) |
| `DATABASE_URL` | `unset` | Fallback when `FUSE_DB_URL` is unset |
| `FUSE_DB_POOL_SIZE` | `1` | SQLite connection pool size |
| `FUSE_CONFIG` | `config.toml` | Config file path |
| `FUSE_HOST` | `127.0.0.1` | HTTP server bind host |
| `FUSE_SERVICE` | `unset` | Select service when multiple are declared |
| `FUSE_MAX_REQUESTS` | `unset` | Stop server after N requests (useful in tests) |
| `FUSE_LOG` | `info` | Minimum log level (`trace`/`debug`/`info`/`warn`/`error`) |
| `FUSE_COLOR` | `auto` | ANSI color (`auto`/`always`/`never`) |
| `NO_COLOR` | `unset` | Disable ANSI color when set |
| `FUSE_REQUEST_LOG` | `unset` | `structured` for JSON request logs on stderr |
| `FUSE_METRICS_HOOK` | `unset` | `stderr` for per-request metrics lines |
| `FUSE_OPENAPI_JSON_PATH` | `unset` | Filesystem path to the OpenAPI JSON served by the built-in UI |
| `FUSE_OPENAPI_UI_PATH` | `unset` | HTTP route prefix for the built-in OpenAPI UI (`/docs` by default) |
| `FUSE_ASSET_MAP` | `unset` | Logical→public URL mappings for `asset()` |
| `FUSE_VITE_PROXY_URL` | `unset` | Forward unknown routes to Vite dev server |
| `FUSE_SVG_DIR` | `unset` | Override SVG base directory for `svg.inline` |
| `FUSE_STATIC_DIR` | `unset` | Serve static files from this directory |
| `FUSE_STATIC_INDEX` | `index.html` | Directory fallback when `FUSE_STATIC_DIR` is set |
| `FUSE_DEV_MODE` | `unset` | Enable development-mode runtime behavior |
| `FUSE_AOT_BUILD_INFO` | `unset` | Print AOT build metadata and exit (AOT binaries only) |
| `FUSE_AOT_STARTUP_TRACE` | `unset` | Emit startup diagnostic line (AOT binaries only) |
| `FUSE_AOT_REQUEST_LOG_DEFAULT` | `unset` | Default to structured request logging in AOT release |

---

## Practical Constraints

- Database: SQLite only (no ORM layer).
- No generics, reflection, macros, or custom operators.
- No inheritance. Use composition and `type X = Y without ...` derivation.
- Task model is structured only; there are no detached tasks or callbacks.
- Redirects are manual; `3xx` responses surface as `http.error` with `code = "http_status"`.

# FUSE Developer Reference

_Auto-generated from `spec/fls.md`, `spec/runtime.md`, and `governance/scope.md` by `scripts/generate_guide_docs.sh`._

This document is the reference for building applications with FUSE.
If you are new to FUSE, start with [Onboarding Guide](onboarding.md) and [Boundary Contracts](boundary-contracts.md) before this reference.

---

## Install and Downloads

Release artifacts are published on GitHub Releases:

- https://github.com/dmitrijkiltau/FUSE/releases

---

## Language at a Glance

Top-level declarations:

- `import`
- `fn`
- `type`
- `enum`
- `config`
- `service`
- `app`
- `migration`
- `test`

Core statements:

- `let` / `var`
- assignment
- `if` / `else`
- `match`
- `for` / `while`
- `break` / `continue`
- `return`

Core expression features:

- null-coalescing: `??`
- optional access: `?.`, `?[idx]`
- bang-chain conversion: `?!`
- ranges: `a..b`
- concurrency forms: `spawn`, `await`, `box`

---
## Types

- `Int`, `Float`, `Bool`, `String`, `Bytes`, `Html`
- `Id`, `Email`
- `Error`
- `List<T>`, `Map<K,V>`, `Option<T>`, `Result<T,E>`
- user-defined `type` and `enum` are nominal

Reserved namespace:

- `std.Error.*` is reserved for standardized runtime error behavior.

Type shorthand:

- `T?` desugars to `Option<T>`.
- `null` is the optional empty value.
- `x ?? y` is null-coalescing.
- `x?.field` and `x?[idx]` are optional access forms.
- `Some` / `None` are valid match patterns.

Result types:

- `T!E` desugars to `Result<T, E>`.
- `T!` is invalid; result types must declare an explicit error domain.
- for function/service return boundaries, each `E` must be a declared nominal `type` or `enum` (including chained forms like `T!AuthError!DbError`)
- `expr ?! err` applies bang-chain error conversion.
- `expr ?!` is propagation-only for `Result<T,E>`; `Option<T> ?!` requires an explicit `err`.

Refinements:

Refinements attach predicates to primitive base types in type positions:

- `String(1..80)`
- `Int(0..130)`
- `Float(0.0..1.0)`
- `String(regex("^[a-z0-9_-]+$"))`
- `String(1..80, regex("^[a-z]"), predicate(is_slug))`

Constraint forms:

- range literals (`1..80`, `0..130`, `0.0..1.0`)
- `regex("<pattern>")` on string-like bases
- `predicate(<fn_ident>)` where the function signature is `fn(<base>) -> Bool`

### Type inference

- local inference for `let` / `var`
- function parameter types are required
- function return type is optional

### Comparison operators

- Equality operators (`==`, `!=`) are defined for same-typed scalar pairs:
  `Int`, `Float`, `Bool`, `String`, and `Bytes`.
- Relational operators (`<`, `<=`, `>`, `>=`) are defined for numeric pairs (`Int`, `Float`).
- Comparisons outside supported operand pairs are invalid.

Runtime error behavior for unsupported pairs is defined in
[Expression operator behavior](runtime.md#expression-operator-behavior).

Type derivation:

`type PublicUser = User without password, secret` creates a new nominal type derived from `User`
with listed fields removed. Field types/defaults are preserved for retained fields.

Base types can be module-qualified (`Foo.User`). Unknown base types or fields are errors.

---

## Strings, Interpolation, and Comments

- Double-quoted strings only (no multiline strings).
- Escapes: `\n`, `\t`, `\r`, `\\`, `\"`. Unknown escapes pass through (`\$` produces `$`).
- Interpolation: `${expr}` inside double quotes.

- Line comment: `# ...`
- Doc comment: `## ...` attaches to the next declaration

---

## Grammar (EBNF approximation)

Top level:

```ebnf
Program        := { RequiresDecl } { TopDecl }
RequiresDecl   := "requires" Capability { "," Capability } NEWLINE
Capability     := "db" | "crypto" | "network" | "time"

TopDecl        := ImportDecl
                | AppDecl
                | ServiceDecl
                | ConfigDecl
                | TypeDecl
                | EnumDecl
                | MigrationDecl
                | TestDecl
                | FnDecl

ImportDecl     := "import" ImportSpec NEWLINE
ImportSpec     := Ident
                | Ident "from" StringLit
                | "{" Ident { "," Ident } "}" "from" StringLit
                | Ident "as" Ident "from" StringLit

TypeDecl       := "type" Ident ":" NEWLINE INDENT { FieldDecl } DEDENT
                | "type" Ident "=" TypeName "without" Ident { "," Ident } NEWLINE
FieldDecl      := Ident ":" TypeRef [ "=" Expr ] NEWLINE

EnumDecl       := "enum" Ident ":" NEWLINE INDENT { EnumVariant } DEDENT
EnumVariant    := Ident [ "(" TypeRef { "," TypeRef } ")" ] NEWLINE

FnDecl         := "fn" Ident "(" NEWLINE* [ ParamList [ "," ] ] NEWLINE* ")" [ "->" TypeRef ] ":" NEWLINE Block
ParamList      := Param { "," NEWLINE* Param }
Param          := Ident ":" TypeRef [ "=" Expr ]

Block          := INDENT { Stmt } DEDENT

Stmt           := LetStmt
                | VarStmt
                | AssignStmt
                | ReturnStmt
                | IfStmt
                | MatchStmt
                | ForStmt
                | WhileStmt
                | TransactionStmt
                | BreakStmt
                | ContinueStmt
                | ExprStmt

LetStmt        := "let" Ident [ ":" TypeRef ] "=" Expr NEWLINE
VarStmt        := "var" Ident [ ":" TypeRef ] "=" Expr NEWLINE
AssignStmt     := LValue "=" Expr NEWLINE
LValue         := Ident | Member | OptionalMember | Index | OptionalIndex
ReturnStmt     := "return" [ Expr ] NEWLINE
ExprStmt       := Expr NEWLINE | SpawnExpr

IfStmt         := "if" Expr ":" IfBody { "else" "if" Expr ":" IfBody } [ "else" ":" IfBody ]
IfBody         := NEWLINE Block | InlineStmt
InlineStmt     := Stmt
MatchStmt      := "match" Expr ":" NEWLINE INDENT { MatchCase } DEDENT
MatchCase      := Pattern ( "->" Expr NEWLINE | ":" NEWLINE Block )
                # `Pattern -> Expr` is sugar for `Pattern: return Expr`
ForStmt        := "for" Pattern "in" Expr ":" NEWLINE Block
WhileStmt      := "while" Expr ":" NEWLINE Block
TransactionStmt := "transaction" ":" NEWLINE Block

AppDecl        := "app" StringLit ":" NEWLINE Block
ServiceDecl    := "service" Ident "at" StringLit ":" NEWLINE INDENT { RouteDecl } DEDENT

RouteDecl      := HttpVerb StringLit [ "body" TypeRef ] "->" TypeRef ":" NEWLINE Block
HttpVerb       := "get" | "post" | "put" | "patch" | "delete"

ConfigDecl     := "config" Ident ":" NEWLINE INDENT { ConfigField } DEDENT
ConfigField    := Ident ":" TypeRef "=" Expr NEWLINE

MigrationDecl  := "migration" ( Ident | StringLit | Int ) ":" NEWLINE Block
TestDecl       := "test" StringLit ":" NEWLINE Block
```

Types:

```ebnf
TypeRef        := TypeAtom { "?" | "!" [ TypeRef ] }
TypeAtom       := TypeName
                | TypeName "<" TypeRef { "," TypeRef } ">"
                | TypeName "(" [ Expr { "," Expr } ] ")"
TypeName       := Ident { "." Ident }
```

Expressions:

```ebnf
Expr           := CoalesceExpr
CoalesceExpr   := OrExpr { "??" OrExpr }
OrExpr         := AndExpr { "or" AndExpr }
AndExpr        := EqExpr  { "and" EqExpr }
EqExpr         := RelExpr { ("==" | "!=") RelExpr }
RelExpr        := RangeExpr { ("<" | "<=" | ">" | ">=") RangeExpr }
RangeExpr      := AddExpr { ".." AddExpr }
AddExpr        := MulExpr { ("+" | "-") MulExpr }
MulExpr        := UnaryExpr { ("*" | "/" | "%") UnaryExpr }
UnaryExpr      := ("-" | "!") UnaryExpr
                | "await" UnaryExpr
                | "box" UnaryExpr
                | PostfixExpr
PostfixExpr    := PrimaryExpr { Call | Member | OptionalMember | Index | OptionalIndex | BangChain }
Call           := "(" [ ArgList ] ")"
ArgList        := Arg { "," Arg }
Arg            := [ Ident "=" ] Expr
Member         := "." Ident
OptionalMember := "?." Ident
Index          := "[" Expr "]"
OptionalIndex  := "?[" Expr "]"
BangChain      := "?!" [ Expr ]
PrimaryExpr    := Literal
                | Ident
                | "(" Expr ")"
                | StructLit
                | ListLit
                | MapLit
                | InterpString
                | SpawnExpr

StructLit      := Ident "(" [ NamedArgs ] ")"
NamedArgs      := Ident "=" Expr { "," Ident "=" Expr }

ListLit        := "[" [ Expr { "," Expr } ] "]"
MapLit         := "{" [ Expr ":" Expr { "," Expr ":" Expr } ] "}"
SpawnExpr      := "spawn" ":" NEWLINE Block

HtmlBlockSuffix := ":" ( NEWLINE INDENT HtmlChildStmt* DEDENT | Expr )
HtmlChildStmt   := Expr NEWLINE
```

Patterns:

```ebnf
Pattern        := "_" | Literal | TypeName [ "(" PatternArgs ")" ]
PatternArgs    := Pattern { "," Pattern }
               | PatternField { "," PatternField }
PatternField   := Ident "=" Pattern
```

Notes:

- `StructLit` is chosen when an identifier call contains named arguments.
- `spawn` is an expression whose block provides its own newline.
- `HtmlBlockSuffix` is enabled only in statement value positions (`let`/`var` RHS, `return` expr,
  assignment RHS, expression statements). It is parsed only for call expressions and lowered to a call
  with block-sugar args (`{}` attrs if omitted, plus `List<Html>` children).
- HTML block children must be expression statements; bare string literals in HTML blocks are lowered to
  `html.text(...)`, while non-literal expressions are not coerced.
- HTML tag calls accept attribute shorthand (`div(class="hero")`) with string literals only; it lowers
  to a standard attrs map argument (`div({"class": "hero"})`).
- Named call args can use keyword names (`type="button"`, `for="x"`), and named args may omit commas
  when separated by layout (`button(class="x" id="y")` split across lines).
- In HTML attribute shorthand, `_` in attribute names is normalized to `-`
  (`aria_label` -> `aria-label`, `data_view` -> `data-view`).
- Postfix chains can continue across line breaks when the next token is a postfix continuation
  (`(`, `.`, `[`, `?`, `?!`), so long call/member/index chains can be wrapped line-by-line.
- Call argument lists allow line breaks and trailing commas before `)`.
- Function parameter lists allow line breaks and a trailing comma before `)`.
- `if` / `else if` / `else` bodies can use either a normal indented block or an inline single statement
  (`if flag: x = 1`).
- `HtmlBlockSuffix` also supports an inline single child expression (`span(): "FUSE"`).

---

## Imports and Modules

`import` declarations are resolved at load time.

- Module imports register an alias for qualified access (`Foo.bar`, `Foo.Config.field`, `Foo.Enum.Variant`).
- Named imports bring specific items into local scope.

Resolution rules:

- `import Foo` loads `Foo.fuse` from the current file directory.
- `import X from "path"` loads `path` relative to current file; `.fuse` is added if missing.
- `import {A, B} from "path"` loads module and imports listed names into local scope.
- `import X as Y from "path"` loads `path` and registers the module under alias `Y` for qualified access.
- `import X from "root:path/to/module"` loads from package root (`fuse.toml` directory); if no manifest is found, root falls back to the entry module directory.

Notes:

- module imports do not automatically import all members into local scope
- named imports do not create a module alias
- function symbols are module-scoped (not global across all loaded modules)
- unqualified function calls resolve in this order: current module, then named imports
- module-qualified calls (`Foo.bar`) resolve against the referenced module alias
- duplicate named imports in one module are load-time errors
- duplicate function names across different modules are allowed
- module-qualified type references are valid in type positions (`Foo.User`, `Foo.Config`)
- dependency modules use `dep:` import paths (for example, `dep:Auth/lib`)
- root-qualified modules use `root:` import paths (for example, `root:lib/auth`)

Module capabilities:

- modules may declare capability requirements with top-level `requires` declarations
- allowed capabilities are `db`, `crypto`, `network`, and `time`
- duplicate capability declarations in one module are semantic errors
- capability checks are compile-time only (no runtime capability guard)
- calls requiring capabilities are rejected when the current module does not declare them
- call sites to imported module functions must declare every capability required by the callee module
  (capability leakage across module boundaries is rejected)
- `transaction` blocks are valid only in modules with `requires db` and no additional capabilities

Strict architecture mode (`--strict-architecture`) adds compile-time architectural checks:

- capability purity: modules must not declare unused capabilities
- cross-layer cycle detection: import graphs that form cycles between logical layers are rejected
- error-domain isolation: a module's function/service boundary signatures must not mix error
  domains from multiple modules

---

## Services and HTTP Contracts

Route syntax uses typed path params inside the route string, for example:

```fuse
get "/users/{id: Id}" -> User:
  ...
```

The `body` keyword introduces the request body type:

```fuse
post "/users" body UserCreate -> User:
  ...
```

Binding/encoding/error semantics for routes are runtime behavior and are defined in `runtime.md`.

HTTP-specific route primitives (`request.header/cookie` and
`response.header/cookie/delete_cookie`) are runtime semantics owned by `runtime.md`.

---

## Static Restrictions

### Spawn static restrictions

Inside a `spawn` block, semantic analysis rejects:

- `box` capture/use (including captured outer boxed bindings)
- runtime side-effect builtins (`db.*`, `serve`, `print`, `input`, `log`, `env`, `asset`, `svg.inline`)
- mutation of captured outer bindings

Structured task lifetime checks are also enforced at compile time:

- detached task expressions are rejected
- spawned task bindings must be awaited before leaving lexical scope
- reassigning a spawned task binding before `await` is rejected

These restrictions are part of the language contract for deterministic cross-backend concurrency.

### Transaction static restrictions

`transaction:` defines a compiler-constrained block for deterministic DB transaction scope.

Inside a `transaction` block, semantic analysis rejects:

- `spawn` expressions
- `await` expressions
- early `return`
- loop control flow (`break` / `continue`)
- capability use outside `db`

Module-level guardrails for `transaction` blocks:

- the containing module must declare `requires db`
- the containing module must not declare non-`db` capabilities

---

## Runtime Behavior

### Expression operator behavior

Comparison behavior is shared across AST/native backends:

- `==` / `!=` support same-typed pairs for `Int`, `Float`, `Bool`, `String`, and `Bytes`.
- `<`, `<=`, `>`, `>=` support numeric pairs (`Int`, `Float`) only.
- unsupported comparison operand pairs produce runtime errors.

### Validation and boundary enforcement

Validation is applied at runtime in these places:

- struct literal construction (`Type(...)`)
- JSON decode for HTTP body
- config loading
- CLI flag binding
- route parameter parsing

There is no global "validate on assignment" mode.

#### Default values

Defaults are applied before validation:

- missing field with default -> default is used
- missing optional field -> `null`
- explicit `null` stays `null` (even if a default exists)

#### Built-in refinements

Refinements support range, regex, and predicate constraints:

- `String(1..80)` length constraint
- `String(regex("^[a-z0-9_-]+$"))` pattern constraint
- `String(1..80, regex("^[a-z]"), predicate(is_slug))` mixed constraints, left-to-right
- `Int(0..130)` numeric range
- `Float(0.0..1.0)` numeric range

Rules:

- `regex("...")` is valid on string-like refined bases (`String`, `Id`, `Email`).
- `predicate(fn_name)` requires a function signature `fn(<base>) -> Bool`.

#### `Id` and `Email`

- `Id` is a non-empty string.
- `Email` uses a simple `local@domain` check with a `.` in the domain.

### JSON behavior

#### Structs

- encode to JSON objects with declared field names
- all fields are included (including defaults)
- `null` represents optional empty value

#### Struct decoding

- missing field with default -> default value
- missing field with no default -> error
- optional fields accept missing or `null`
- unknown fields -> error

#### Enums

Enums use a tagged object format:

```json
{ "type": "Variant", "data": ... }
```

Rules:

- no payload: omit `data`
- single payload: `data` is the value
- multiple payloads: `data` is an array

#### Built-in types and generics

- `String`, `Id`, `Email` -> JSON string
- `Bytes` -> JSON base64 string (standard alphabet with `=` padding)
- `Html` -> JSON string via `html.render(...)` output
- `Bool`, `Int`, `Float` -> JSON number/bool
- `List<T>` -> JSON array
- `Map<K,V>` -> JSON object (runtime requires `K = String`)
- user-defined `struct` and `enum` decode with same validation model as struct literals
- `Result<T,E>` -> tagged object:
  - `{"type":"Ok","data":...}` decodes as `Ok(T)`
  - `{"type":"Err","data":...}` decodes as `Err(E)`

`Bytes` use base64 text at JSON/config/CLI boundaries. Runtime values are raw bytes.
`Html` values are runtime trees and are not parsed from config/env/CLI.

### Errors and HTTP status mapping

The runtime recognizes a small set of error struct names for standardized HTTP status mapping
and error JSON formatting.

Preferred canonical names (from `std.Error`):

- `std.Error.Validation`
- `std.Error`
- `std.Error.BadRequest`
- `std.Error.Unauthorized`
- `std.Error.Forbidden`
- `std.Error.NotFound`
- `std.Error.Conflict`

Compatibility short names are also recognized (`Validation`, `Error`, `BadRequest`,
`Unauthorized`, `Forbidden`, `NotFound`, `Conflict`), which commonly occur after named imports.
Other names do not participate in standardized mapping/formatting behavior.

### Error JSON shape

Errors are rendered as JSON with a single `error` object:

```json
{
  "error": {
    "code": "validation_error",
    "message": "validation failed",
    "fields": [
      { "path": "email", "code": "invalid_value", "message": "invalid email address" }
    ]
  }
}
```

Rules:

- `std.Error.Validation` / `Validation` uses `message` and `fields`
  (list of structs with `path`, `code`, `message`).
- `std.Error` / `Error` uses `code` and `message`. Other fields are ignored for JSON output.
- `std.Error.BadRequest` / `BadRequest`, `std.Error.Unauthorized` / `Unauthorized`,
  `std.Error.Forbidden` / `Forbidden`, `std.Error.NotFound` / `NotFound`,
  `std.Error.Conflict` / `Conflict` use their `message` field if present, otherwise a default message.
- Any other error value renders as `internal_error`.

Status mapping uses the error name first, then `std.Error.status` if present:

- `std.Error.Validation` / `Validation` -> 400
- `std.Error.BadRequest` / `BadRequest` -> 400
- `std.Error.Unauthorized` / `Unauthorized` -> 401
- `std.Error.Forbidden` / `Forbidden` -> 403
- `std.Error.NotFound` / `NotFound` -> 404
- `std.Error.Conflict` / `Conflict` -> 409
- `std.Error` / `Error` with `status: Int` -> that status
- anything else -> 500

`expr ?! err` behavior:

- `T!E` is `Result<T, E>`.
- `T!` is a compile-time error (explicit error domains are required).
- for function/service return boundaries, each error domain must be a declared nominal `type` or `enum`

`expr ?! err` rules:

- If `expr` is `Option<T>` and is `None`, return `Err(err)`.
- If `expr` is `Result<T, E>` and is `Err`, replace the error with `err`.
- If `expr ?!` omits `err`, `Result` propagates the existing error.
- `Option<T> ?!` without an explicit `err` is a compile-time error.

### Config and CLI binding

Config values resolve in this order:

1. environment variables (override config file)
2. config file (default `config.toml`, overridable via `FUSE_CONFIG`)
3. default expressions

The `fuse` CLI also loads `.env` from the package directory (if present) and injects any missing
variables before this resolution. Existing environment variables are never overridden by `.env`.

Config file format is a minimal TOML-like subset:

```toml
[App]
port = 3000
dbUrl = "sqlite://app.db"
```

Notes:

- only section headers and `key = value` pairs are supported
- values are parsed as strings (with basic `"` escapes), then converted using env-var conversion rules

Env override naming derives from config and field names:

- `App.port` -> `APP_PORT`
- `dbUrl` -> `DB_URL`
- hyphens become underscores; camelCase splits to `SNAKE_CASE`

Type support levels for config values (env and file values):

- **Full**: scalars (`Int`, `Float`, `Bool`, `String`, `Id`, `Email`, `Bytes`) and `Option<T>`.
- **Structured via JSON text**: `List<T>`, `Map<String,V>`, user-defined `struct`, user-defined `enum`.
- **Rejected**: `Html`, `Map<K,V>` where `K != String`, `Result<T,E>`.

Compatibility notes:

- `Bytes` must be valid base64 text; invalid base64 is a validation error.
- for structured values, parse failures (invalid JSON/type mismatch/unknown field) surface as
  validation errors on the target field path.

CLI binding:

CLI binding is enabled when program args are passed after the file (or after `--`):

```bash
fusec --run file.fuse -- --name=Codex
```

Rules:

- flags only (no positional arguments)
- `--flag value` and `--flag=value` are supported
- `--flag` sets `Bool` to `true`; `--no-flag` sets it to `false`
- unknown flags are validation errors
- multiple values for the same flag are rejected
- binding calls `fn main` from the root module directly; `app` block is ignored when program args are present

Type support levels mirror config/env parsing:

- **Full**: scalar types and `Option<T>`.
- **Structured via JSON text**: `List<T>`, `Map<String,V>`, user-defined `struct`, user-defined `enum`.
- **Rejected**: `Html`, `Map<K,V>` with non-`String` keys, `Result<T,E>`.

For `Bytes`, CLI values must be base64 text.

Validation errors are printed as JSON on stderr and usually exit with code 2.

`fuse` CLI wrapper output contract (`check|run|build|test`):

- emits stderr step markers: `[command] start` and `[command] ok|failed|validation failed`
- keeps JSON validation payloads uncolored/machine-readable
- `run` CLI argument validation failures exit with code `2`

---

## HTTP Runtime

### Routing

- paths are split on `/` and matched segment-by-segment
- route params use `{name: Type}` and must occupy the whole segment
- params parse with env-like scalar/optional/refined rules
- `body` introduces a JSON request body bound to `body` in the handler

### Response

- successful values encode as JSON with `Content-Type: application/json` by default
- if route return type is `Html` (or `Result<Html, E>` on success), response is rendered once with
  `Content-Type: text/html; charset=utf-8`
- route handlers may append response headers via `response.header(name, value)`
- route handlers may manage cookies via `response.cookie(name, value)` and
  `response.delete_cookie(name)` (emitted as `Set-Cookie` headers)
- `Result` errors are mapped using the status rules above
- unsupported HTTP methods return `405` with `internal_error` JSON
- no HTMX-specific runtime mode: HTMX-style flows are ordinary `Html` route returns

### Request primitives

- route handlers may read inbound headers with `request.header(name)` (case-insensitive)
- route handlers may read cookie values with `request.cookie(name)`
- `request.*` and `response.*` primitives are only valid while evaluating an HTTP route handler

### Observability baseline

Request ID propagation:

- each HTTP request resolves one request ID with precedence:
  1. inbound `x-request-id` (if valid)
  2. inbound `x-correlation-id` (if valid)
  3. runtime-generated ID (`req-<hex>`)
- runtime normalizes the resolved value into request headers, so
  `request.header("x-request-id")` returns the lifecycle request ID inside route handlers
- runtime emits `X-Request-Id` on HTTP responses for runtime-owned handlers and runtime-generated
  status/error responses
- Vite proxy fallback forwards `X-Request-Id` upstream and injects it into the proxied response

Structured request logging mode:

- opt-in via `FUSE_REQUEST_LOG=structured` (`1`/`true` are also accepted)
- emits one JSON line to stderr per handled request with stable fields:
  `event`, `runtime`, `request_id`, `method`, `path`, `status`, `duration_ms`, `response_bytes`
- disabled by default; does not change runtime semantics
- release AOT binaries support optional default posture:
  if `FUSE_AOT_REQUEST_LOG_DEFAULT` is truthy and `FUSE_REQUEST_LOG` is unset,
  runtime sets `FUSE_REQUEST_LOG=structured` before startup

Metrics hook extension point (non-semantic):

- opt-in via `FUSE_METRICS_HOOK=stderr`
- emits one line per handled request on stderr as:
  `metrics: <json>`
- stable JSON fields:
  `metric` (`http.server.request`), `runtime`, `request_id`, `method`, `path`, `status`,
  `duration_ms`
- unsupported/empty hook values are treated as no-op
- hook emission is best-effort and must not change request/response behavior

Deterministic panic taxonomy:

- fatal envelope class remains `runtime_fatal` for handled runtime errors and `panic` for
  process-level panics
- `panic` envelope messages include `panic_kind=<panic_static_str|panic_string|panic_non_string>`
  for deterministic panic payload classification

Production health route convention (non-built-in):

- runtime does not auto-register `/health`.
- canonical minimal route pattern is:
  `get "/health" -> Map<String, String>: return {"status": "ok"}`
- production guidance should treat this pattern as the default liveness/readiness contract unless a
  service-specific contract is documented.

Explicit non-goal:

- no runtime plugin extension system (no runtime-loaded plugin/module capability).

---

## Builtins

- `print(value)` prints stringified value to stdout
- `input(prompt: String = "") -> String` prints optional prompt and reads one line from stdin
- `log(...)` writes log lines to stderr (see Logging)
- `db.exec/query/one` execute SQL against configured DB
- `db.from(table)` builds parameterized queries
- `transaction:` opens a constrained DB transaction scope (`BEGIN`/`COMMIT`/`ROLLBACK`)
- `assert(cond, message?)` throws runtime error when `cond` is false
- `env(name: String) -> String?` returns env var or `null`
- `asset(path: String) -> String` resolves to hashed/static public URL when asset map is configured
- `serve(port)` starts HTTP server on `FUSE_HOST:port`
- `request.header(name: String) -> String?` reads inbound HTTP headers
- `request.cookie(name: String) -> String?` reads inbound HTTP cookie values
- `response.header(name: String, value: String)` appends response headers
- `response.cookie(name: String, value: String)` appends HTTP-only session cookies
- `response.delete_cookie(name: String)` emits cookie expiration headers
- HTML tag builtins (`html`, `head`, `body`, `div`, `meta`, `button`, ...)
- `html.text`, `html.raw`, `html.node`, `html.render`
- `svg.inline(path: String) -> Html`
- `json.encode(value) -> String` serializes a value to a JSON string
- `json.decode(text: String) -> Value` parses a JSON string into a runtime value

`input` behavior notes:

- prompt text is written without a trailing newline
- trailing `\n`/`\r\n` is trimmed from the returned line
- in non-interactive mode with no stdin data, runtime fails with:
  `input requires stdin data in non-interactive mode`
- `input()` / `input("...")` resolve to the CLI input builtin; HTML input tags remain available
  through tag-form calls such as `input(type="text")`

Compile-time sugar affecting HTML builtins:

- HTML block syntax (`div(): ...`) lowers to normal calls with explicit attrs + `List<Html>` children
- bare string literals in HTML blocks lower to `html.text(...)`
- attribute shorthand (`div(class="hero")`) lowers to attrs maps

---

## Database (SQLite)

Database access is intentionally minimal and currently uses SQLite via a pooled set of
connections.

Configuration sources:

- `FUSE_DB_URL` (preferred) or `DATABASE_URL`
- `App.dbUrl` if config has been loaded
- `FUSE_DB_POOL_SIZE` (default `1`) for pool sizing
- `App.dbPoolSize` as optional fallback when `FUSE_DB_POOL_SIZE` is unset

URL format:

- `sqlite://path` or `sqlite:path`

Builtins:

- `db.exec(sql, params?)` executes SQL batch (no return value)
- `db.query(sql, params?)` returns `List<Map<String, Value>>`
- `db.one(sql, params?)` returns first row map or `null`
- `db.from(table)` returns `Query` builder
- `transaction:` opens a transaction, executes its block, commits on success, and rolls back on
  block failure

Query builder methods (immutable style; each returns a new `Query`):

- `Query.select(columns)`
- `Query.where(column, op, value)`
- `Query.order_by(column, dir)` where `dir` is `asc`/`desc`
- `Query.limit(n)` where `n >= 0`
- `Query.one()`
- `Query.all()`
- `Query.exec()`
- `Query.sql()` and `Query.params()` for inspection/debugging

Parameter binding:

- SQL uses positional `?` placeholders with `List` params
- supported param types: `null`, `Int`, `Float`, `Bool`, `String`, `Bytes`
  (boxed/results are unwrapped)
- `in` expects non-empty list and expands to `IN (?, ?, ...)`

Identifier constraints:

- table/column names must be identifiers (`col` or `table.col`)
- `where` operators: `=`, `!=`, `<`, `<=`, `>`, `>=`, `like`, `in` (case-insensitive)
- `order_by` direction: `asc` or `desc`

Value mapping:

- `NULL` -> `null`
- integers -> `Int`
- reals -> `Float`
- text -> `String`
- blobs -> `Bytes`

Connection pool behavior:

- DB calls use pooled SQLite connections.
- the active connection is pinned for migration and `transaction:` scopes (`BEGIN`/`COMMIT`/`ROLLBACK`).
- pool-size values must be integer `>= 1`; invalid values report runtime/config errors.

### Migrations

`migration <name>:` declares a migration block.

Run migrations with:

```bash
fusec --migrate path/to/file.fuse
```

Rules:

- migrations are collected from all loaded modules
- run order is ascending by migration name
- applied migrations are tracked in `__fuse_migrations`
- only up migrations exist today (no down/rollback)
- migrations execute via AST interpreter

### Tests

`test "name":` declares a test block.

Run tests with:

```bash
fusec --test path/to/file.fuse
```

Rules:

- tests are collected from all loaded modules
- run order is ascending by test name
- tests execute via AST interpreter
- failures report non-zero exit

---

## Concurrency

`spawn:` creates a task and returns `Task<T>` where `T` is block result.
Spawned tasks run on a shared worker pool. Execution is asynchronous relative to the caller
and may overlap with other spawned tasks.

`await expr` waits on a task and yields its result.

Structured concurrency is enforced at compile time:

- detached task expressions are invalid
- spawned task bindings must be awaited before scope exit
- spawned task bindings cannot be reassigned before `await`
- `transaction:` blocks reject `spawn` and `await`

Task surface (v0.2.0):

- `Task<T>` remains an opaque runtime type
- task helper builtins were removed (`task.id`, `task.done`, `task.cancel`)
- task values are consumed via `await` only

Spawn determinism restrictions are enforced at compile time by semantic analysis.
See [Spawn static restrictions](fls.md#spawn-static-restrictions-v020) for the full list.

`box expr` creates a shared mutable cell. Boxed values are transparently dereferenced in most
expressions; assigning boxed bindings updates shared cell state. `spawn` blocks cannot capture or
use boxed state.

---

## Loops, Indexing, and Ranges

### Loops

- `for` iterates over `List<T>` and `Map<K, V>` values (map iteration yields values)
- `break` exits nearest loop
- `continue` skips to next iteration

### Indexing

- `list[idx]` reads list element; `idx` must be in-bounds `Int`
- out-of-bounds list access raises runtime error
- `map[key]` reads map element; missing key yields `null`

Assignment targets allow:

- `list[idx] = value` (bounds-checked)
- `map[key] = value` (insert/overwrite)

Optional access in assignment targets (for example `foo?.bar = x`, `items?[0] = x`) errors when base is `null`.

### Ranges

`a..b` evaluates to inclusive numeric `List`.

- only numeric bounds are allowed
- if `a > b`, runtime error
- float ranges step by `1.0` (for example `1.5..3.5` -> `[1.5, 2.5, 3.5]`)

---

## Logging

`log` is a minimal runtime logging builtin shared by all backends.

Usage:

- `log("message")` logs at `INFO`
- `log("warn", "message")` logs at `WARN`
- if there are 2+ args and first arg is known level (`trace`, `debug`, `info`, `warn`, `error`),
  it is treated as level; the rest are stringified and joined with spaces
- if there is at least one extra argument after the message, `log` emits JSON

Output:

- `[LEVEL] message` to stderr
  (`LEVEL` token may be ANSI-colored; honors `FUSE_COLOR=auto|always|never` and `NO_COLOR`)
- JSON logs are emitted as a single stderr line

Filtering:

- `FUSE_LOG` sets minimum level (default `info`)

Structured logging:

- `log("info", "message", data)` emits JSON:
  `{"level":"info","message":"message","data":<json>}`
- if multiple data values are provided, `data` is a JSON array

---

## Tooling and Package Commands

Common package commands:

- `fuse check`
- `fuse run`
- `fuse dev`
- `fuse test`
- `fuse build`

Compiler/runtime CLI operations include:

- `fusec --check`
- `fusec --run`
- `fusec --test`
- `fusec --migrate`
- `fusec --openapi`

`fuse.toml` sections commonly used:

- `[package]`
- `[build]`
- `[serve]`
- `[assets]`, `[assets.hooks]`
- `[vite]`
- `[dependencies]`

---

## Runtime Environment Variables

| Variable | Default | Description |
|---|---|---|
| `FUSE_DB_URL` | — | Database connection URL (`sqlite://path`) |
| `DATABASE_URL` | — | Fallback DB URL when `FUSE_DB_URL` is unset |
| `FUSE_DB_POOL_SIZE` | `1` | SQLite connection pool size |
| `FUSE_CONFIG` | `config.toml` | Config file path |
| `FUSE_HOST` | `127.0.0.1` | HTTP server bind host |
| `FUSE_SERVICE` | — | Selects service when multiple are declared |
| `FUSE_MAX_REQUESTS` | — | Stop server after N requests (useful for tests) |
| `FUSE_LOG` | `info` | Minimum log level (`trace`, `debug`, `info`, `warn`, `error`) |
| `FUSE_COLOR` | `auto` | ANSI color mode (`auto`, `always`, `never`) |
| `NO_COLOR` | — | Disables ANSI color when set (any value) |
| `FUSE_REQUEST_LOG` | — | Set to `structured` (or `1`/`true`) for JSON request logging on stderr |
| `FUSE_METRICS_HOOK` | — | Set to `stderr` for per-request metrics lines |
| `FUSE_DEV_RELOAD_WS_URL` | — | Enables dev HTML script injection (`/__reload` client) |
| `FUSE_OPENAPI_JSON_PATH` | — | Enables built-in OpenAPI JSON endpoint at this path |
| `FUSE_OPENAPI_UI_PATH` | — | Enables built-in OpenAPI UI at this path |
| `FUSE_ASSET_MAP` | — | Logical-path to public-URL mappings for `asset(path)` |
| `FUSE_VITE_PROXY_URL` | — | Fallback proxy for unknown routes to Vite dev server |
| `FUSE_SVG_DIR` | — | Override SVG base directory for `svg.inline` |
| `FUSE_STATIC_DIR` | — | Serve static files from this directory |
| `FUSE_STATIC_INDEX` | `index.html` | Fallback file for directory requests when `FUSE_STATIC_DIR` is set |
| `FUSE_DEV_MODE` | — | Enables development-mode runtime behavior |
| `FUSE_AOT_BUILD_INFO` | — | Print AOT build metadata and exit (AOT binaries only) |
| `FUSE_AOT_STARTUP_TRACE` | — | Emit startup diagnostic line (AOT binaries only) |
| `FUSE_AOT_REQUEST_LOG_DEFAULT` | — | Default to structured request logging in release AOT binaries |

---

## Constraints

Current practical constraints:

- SQLite-focused database runtime
- no full ORM layer
- task model is still evolving
- native backend uses Cranelift JIT

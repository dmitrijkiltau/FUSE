# Formal language specification

## Lexing + indentation rules

### Tokens

* Identifiers: `[A-Za-z_][A-Za-z0-9_]*`
* Keywords: `app, service, at, get, post, put, patch, delete, fn, type, enum, let, var, return, if, else, match, case, for, in, while, break, continue, import, from, as, config, migration, table, test`
* Literals: `Int`, `Float`, `String`, `Bool`, `Null`
* Operators (MVP):
  `=, ==, !=, <, <=, >, >=, +, -, *, /, %, ., :, ?, !, ??, ?!, ->, =>`

### Significant indentation

FUSE uses Python-style block structure but with stricter rules.

* Indentation is measured in **spaces only** (tabs are illegal).
* Indent width is not fixed, but **must be consistent within a file**.
* A block starts after `:` at end of line.
* New indentation level must be strictly greater than previous.
* Dedent closes blocks until indentation matches a previous level.
* Empty lines ignored.
* Lines inside parentheses/brackets/braces ignore indentation semantics (implicit line joining).

**INDENT/DEDENT algorithm (formal-ish):**

* Maintain a stack `indents` starting with `[0]`
* For each logical line not inside `()[]{}`:

  * Let `col` be count of leading spaces
  * If `col > top(indents)`: emit `INDENT`, push `col`
  * If `col < top(indents)`: while `col < top(indents)` emit `DEDENT`, pop; error if `col != top(indents)` after popping
  * Else: continue

### Comments

* Line comment: `# ...`
* Doc comment: `## ...` attaches to next declaration (for generated docs)

## Grammar (EBNF-ish)

Top level:

```
Program        := { TopDecl }

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
ImportSpec     := Ident { "," Ident }
                | Ident "from" StringLit
                | "{" Ident { "," Ident } "}" "from" StringLit
                | Ident "as" Ident "from" StringLit

TypeDecl       := "type" Ident ":" NEWLINE INDENT { FieldDecl } DEDENT
FieldDecl      := Ident ":" TypeRef [ "=" Expr ] NEWLINE

EnumDecl       := "enum" Ident ":" NEWLINE INDENT { EnumVariant } DEDENT
EnumVariant    := Ident [ "(" TypeRef { "," TypeRef } ")" ] NEWLINE

FnDecl         := "fn" Ident "(" [ ParamList ] ")" [ "->" TypeRef ] ":" NEWLINE Block
ParamList      := Param { "," Param }
Param          := Ident ":" TypeRef [ "=" Expr ]

Block          := INDENT { Stmt } DEDENT

Stmt           := LetStmt
                | VarStmt
                | AssignStmt
                | ReturnStmt
                | IfStmt
                | MatchStmt
                | ExprStmt
                | ForStmt
                | WhileStmt
                | BreakStmt
                | ContinueStmt

LetStmt        := "let" Ident [ ":" TypeRef ] "=" Expr NEWLINE
VarStmt        := "var" Ident [ ":" TypeRef ] "=" Expr NEWLINE
AssignStmt     := LValue "=" Expr NEWLINE
ReturnStmt     := "return" [ Expr ] NEWLINE
ExprStmt       := Expr NEWLINE

IfStmt         := "if" Expr ":" NEWLINE Block { "else" "if" Expr ":" NEWLINE Block } [ "else" ":" NEWLINE Block ]
MatchStmt      := "match" Expr ":" NEWLINE INDENT { "case" Pattern ":" NEWLINE Block } DEDENT

AppDecl        := "app" StringLit ":" NEWLINE Block
ServiceDecl    := "service" Ident "at" StringLit ":" NEWLINE INDENT { RouteDecl } DEDENT

RouteDecl      := HttpVerb StringLit [ "body" TypeRef ] "->" TypeRef ":" NEWLINE Block
HttpVerb       := "get" | "post" | "put" | "patch" | "delete"

ConfigDecl     := "config" Ident ":" NEWLINE INDENT { ConfigField } DEDENT
ConfigField    := Ident ":" TypeRef "=" Expr NEWLINE

TestDecl       := "test" StringLit ":" NEWLINE Block
```

Expressions (minimal):

```
Expr           := CoalesceExpr
CoalesceExpr   := OrExpr { "??" OrExpr }
OrExpr         := AndExpr { "or" AndExpr }        # optional keyword operators
AndExpr        := EqExpr  { "and" EqExpr }
EqExpr         := RelExpr { ("==" | "!=") RelExpr }
RelExpr        := AddExpr { ("<" | "<=" | ">" | ">=") AddExpr }
AddExpr        := MulExpr { ("+" | "-") MulExpr }
MulExpr        := UnaryExpr { ("*" | "/" | "%") UnaryExpr }
UnaryExpr      := ("-" | "!") UnaryExpr | PostfixExpr
PostfixExpr    := PrimaryExpr { Call | Member | OptionalChain | BangChain }
Call           := "(" [ ArgList ] ")"
ArgList        := Expr { "," Expr }
Member         := "." Ident
OptionalChain  := "?." Ident               # optional: safe member access
BangChain      := "?!"
PrimaryExpr    := Literal
                | Ident
                | "(" Expr ")"
                | StructLit
                | ListLit
                | MapLit
                | InterpString

StructLit      := Ident "(" [ NamedArgs ] ")"
NamedArgs      := Ident "=" Expr { "," Ident "=" Expr }

ListLit        := "[" [ Expr { "," Expr } ] "]"
MapLit         := "{" [ Expr ":" Expr { "," Expr ":" Expr } ] "}"
```

(You can evolve this later without breaking the core model.)

---

## AST model (structural spec)

Here’s a clean AST shape that doesn’t collapse into spaghetti.

**Program**

* `items: Vec<Item>`

**Item**

* `Import(ImportDecl)`
* `Type(TypeDecl)`
* `Enum(EnumDecl)`
* `Fn(FnDecl)`
* `Service(ServiceDecl)`
* `Config(ConfigDecl)`
* `App(AppDecl)`
* `Migration(MigrationDecl)` (non-MVP ok)
* `Test(TestDecl)`

**TypeDecl**

* `name: Ident`
* `fields: Vec<FieldDecl>`
* `doc: Option<Doc>`

**FieldDecl**

* `name: Ident`
* `ty: TypeRef`
* `default: Option<Expr>`

**EnumDecl**

* `name`
* `variants: Vec<Variant>`

**Variant**

* `name`
* `payload: Vec<TypeRef>` (tuple-like payload, MVP)

**FnDecl**

* `name`
* `params: Vec<Param>`
* `ret: Option<TypeRef>`
* `body: Block`

**ServiceDecl**

* `name`
* `base_path: String`
* `routes: Vec<RouteDecl>`

**RouteDecl**

* `verb: HttpVerb`
* `path: String` (contains `{param: Type}`)
* `body_type: Option<TypeRef>`
* `ret_type: TypeRef`
* `body: Block`

**Stmt**

* `Let(name, opt_ty, expr)`
* `Var(name, opt_ty, expr)`
* `Assign(lvalue, expr)`
* `Return(opt_expr)`
* `If(cond, then, else_if: Vec<(cond, block)>, else_block)`
* `Match(expr, cases: Vec<(Pattern, Block)>)`
* `For(pattern, iterable, block)` (later)
* `While(cond, block)` (later)
* `Expr(expr)`
* `Break`, `Continue`

**Expr**

* `Literal(...)`
* `Ident(name)`
* `Binary(op, left, right)`
* `Unary(op, expr)`
* `Call(callee, args)`
* `Member(base, name)`
* `StructLit(name, fields)`
* `ListLit(items)`
* `MapLit(pairs)`
* `InterpString(parts)`
* `Coalesce(left, right)`
* `OptionalMember(base, name)` (for `?.`)
* `BangChain(expr)` (for `?!` sugar as an operator node)

**Pattern**

* `Wildcard`
* `Literal`
* `IdentBind(name)`
* `EnumVariant(name, subpatterns...)` (later)
* `StructPattern` (later)

## Type system

### Base types

* `Int`, `Float`, `Bool`, `String`
* `List<T>`, `Map<K,V>`
* `Bytes`
* `Id` (built-in nominal type backed by `String` or `UUID` depending on runtime)
* `Email` (refined String preset)

### Nominal user types

* `type User: ...` creates nominal struct type
* `enum X: ...` creates nominal tagged union

### Refined types

Refinements attach predicates to a base type.

Syntax examples:

* `String(1..80)` length constraint
* `Int(0..130)` numeric range
* `String(regex="...")` (optional later)

**Semantics:**

* Refinements are checked:

  * at decode boundaries (JSON, CLI, HTTP request)
  * at constructors (`User(...)`)
  * optionally at assignment if runtime checks enabled (dev mode)

**Compile-time vs runtime:**

* Compile-time enforces that you can’t pretend a `String` is a `String(1..80)` without validation.
* Runtime does the actual check.

Introduce a compiler-known function:

* `refine<TRefined>(value: TBase) -> TRefined!ValidationError`

### Optionals

`T?` is `Option<T>`.

Rules:

* Can be `null` in JSON input.
* Member access on optional requires:

  * unwrap (`x?` style) or safe access (`x?.field`)
* FUSE supports:

  * `x ?? y` (coalesce)
  * `x?.field` (safe member, returns optional)

### Fallible results

MVP: `T!` is `Result<T, Error>` where `Error` is a built-in nominal base error.
Better: `T!E` is `Result<T, E>`.

I recommend **supporting both**:

* `User!` means `Result<User, Error>`
* `User!NotFound` means `Result<User, NotFound>`

**Propagating errors**

* `expr ?! ErrValue` means:

  * if `expr` is `Option<T>` and is `None`, return `Err(ErrValue)`
  * if `expr` is `Result<T,E>` and is `Err`, map/replace error with `ErrValue` (or wrap)
* `try expr` (optional keyword later) or postfix `!`/`?` can be added, but MVP keeps just `?!` + explicit matching.

### Type inference

* Local inference for `let x = ...`
* Function param/return types required unless trivially inferrable? MVP can require return types for exported functions, optional for local.

### Structural vs nominal

* User-defined `type` is nominal: `User` is not interchangeable with another identical struct.
* Records/anonymous structs are not MVP.

### “without” type projection

`User without id` creates a derived struct type:

* name is anonymous or compiler-generated: `User_without_id`
* fields are copied except excluded
* keeps refinements and defaults

This is crucial for DTOs and kills boilerplate properly.

## Module + import semantics

### Files and modules

* Each file is a module.
* Module name defaults to file name (sans extension).
* Directory modules are supported with `mod.fuse` (optional later). MVP: flat modules is enough.

### Import forms

* `import X` imports module `X` from project or std.
* `import {A, B} from "net/http"` imports named exports from a module path.
* `import X as Y from "foo"` aliasing.

### Export rules

Simple and strict:

* Top-level `type/enum/fn/service/config/app/test` are **exported by default** from their module.
* Prefix with `_` to make private: `_helperFn`.
* Or allow `pub` later. MVP: underscore rule is fine.

### Resolution order

1. Relative: `from "./foo"` (explicit)
2. Project modules
3. Standard library modules (`net/http`, `json`, `log`, etc.)
4. Dependencies from `fuse.toml`

### Cycles

* Module import cycles are allowed only if they don’t require evaluating top-level values (which MVP doesn’t really have anyway).
* Since top-level expressions are disallowed in MVP (except declarations), cycle handling is easy.

## “service” and parameter binding (formal-ish)

Route path params have typed bindings:

Example:
`get "/users/{id: Id}" -> User: ...`

Rules:

* `{name: Type}` declares a parameter.
* Compiler generates:

  * extraction from URL segment
  * parsing + refinement
  * availability of `id` as a local variable of type `Id` inside handler body

For request body:
`post "/users" body UserCreate -> User: ...`

* Body is decoded as JSON into `UserCreate`
* Validation occurs automatically
* The variable name inside handler is `body` (reserved) unless specified later.

Error mapping:

* Any `Result` errors returned by handler are mapped to HTTP status codes by convention:

  * `NotFound` -> 404
  * `ValidationError` -> 400
  * `Unauthorized` -> 401
  * fallback -> 500

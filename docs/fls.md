# Formal language specification (current parser)

This document mirrors the parser + semantic analysis in `crates/fusec`. Where runtime support is
incomplete, the notes call it out explicitly.

---

## Lexing + indentation rules

### Tokens

* Identifiers: `[A-Za-z_][A-Za-z0-9_]*`
* Keywords:
  `app, service, at, get, post, put, patch, delete, fn, type, enum, let, var, return, if, else,
  match, for, in, while, break, continue, import, from, as, config, migration, table, test,
  body, and, or, without, spawn, await, box`
* Literals:
  * integers (`123`)
  * floats (`3.14`)
  * strings (`"hello"`)
  * booleans (`true`, `false`)
  * `null`
* Punctuation/operators:
  `(` `)` `[` `]` `{` `}` `,` `:` `.` `..` `->` `=>` `=` `==` `!=` `<` `<=` `>` `>=`
  `+` `-` `*` `/` `%` `?` `!` `??` `?!`

### Strings + interpolation

* Double-quoted strings only (no multiline strings).
* Escapes: `\n`, `\t`, `\r`, `\\`, `\"`. Unknown escapes pass through (`\$` produces `$`).
* Interpolation: `${expr}` inside double quotes.

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
* Doc comment: `## ...` attaches to the next declaration

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
                | "type" Ident "=" TypeName "without" Ident { "," Ident } NEWLINE
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
                | ForStmt
                | WhileStmt
                | BreakStmt
                | ContinueStmt
                | ExprStmt

LetStmt        := "let" Ident [ ":" TypeRef ] "=" Expr NEWLINE
VarStmt        := "var" Ident [ ":" TypeRef ] "=" Expr NEWLINE
AssignStmt     := LValue "=" Expr NEWLINE
LValue         := Ident | Member | OptionalMember | Index | OptionalIndex
ReturnStmt     := "return" [ Expr ] NEWLINE
ExprStmt       := Expr NEWLINE | SpawnExpr

IfStmt         := "if" Expr ":" NEWLINE Block { "else" "if" Expr ":" NEWLINE Block } [ "else" ":" NEWLINE Block ]
MatchStmt      := "match" Expr ":" NEWLINE INDENT { MatchCase } DEDENT
MatchCase      := Pattern ( "->" Expr NEWLINE | ":" NEWLINE Block )
                # `Pattern -> Expr` is sugar for `Pattern: return Expr`
ForStmt        := "for" Pattern "in" Expr ":" NEWLINE Block
WhileStmt      := "while" Expr ":" NEWLINE Block

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

```
TypeRef        := TypeAtom { "?" | "!" [ TypeRef ] }
TypeAtom       := TypeName
                | TypeName "<" TypeRef { "," TypeRef } ">"
                | TypeName "(" [ Expr { "," Expr } ] ")"
TypeName       := Ident { "." Ident }
```

Expressions:

```
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
```

Patterns:

```
Pattern        := "_" | Literal | Ident [ "(" PatternArgs ")" ]
PatternArgs    := Pattern { "," Pattern }
               | PatternField { "," PatternField }
PatternField   := Ident "=" Pattern
```

Notes:

* `StructLit` is chosen when an identifier call contains named arguments.
* `spawn` is an expression whose block provides its own newline.
* Postfix chains can continue across line breaks when the next token is a postfix continuation
  (`(`, `.`, `[`, `?`, `?!`), so long call/member/index chains can be wrapped line-by-line.
* Call argument lists allow line breaks and trailing commas before `)`.

---

## AST model (structural spec)

The AST matches `crates/fusec/src/ast.rs`:

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
* `Migration(MigrationDecl)`
* `Test(TestDecl)`

**Decls**

* `TypeDecl { name, fields, derive, doc }`
* `TypeDerive { base, without }`
* `FieldDecl { name, ty, default }`
* `EnumDecl { name, variants, doc }`
* `EnumVariant { name, payload }`
* `FnDecl { name, params, ret, body, doc }`
* `ServiceDecl { name, base_path, routes, doc }`
* `RouteDecl { verb, path, body_type, ret_type, body }`
* `ConfigDecl { name, fields, doc }`
* `ConfigField { name, ty, value }`
* `AppDecl { name, body, doc }`
* `MigrationDecl { header, body, doc }`
* `TestDecl { name, body, doc }`

**Stmt**

* `Let { name, ty, expr }`
* `Var { name, ty, expr }`
* `Assign { target, expr }`
* `Return { expr }`
* `If { cond, then_block, else_if, else_block }`
* `Match { expr, cases }`
* `For { pat, iter, block }`
* `While { cond, block }`
* `Expr(expr)`
* `Break`, `Continue`

**Expr**

* `Literal`
* `Ident`
* `Binary(op, left, right)`
* `Unary(op, expr)`
* `Call(callee, args)`
* `Member(base, name)`
* `OptionalMember(base, name)`
* `StructLit(name, fields)`
* `ListLit(items)`
* `MapLit(pairs)`
* `InterpString(parts)`
* `Coalesce(left, right)`
* `BangChain(expr, error?)`
* `Spawn(block)`
* `Await(expr)`
* `Box(expr)`

**Pattern**

* `Wildcard`
* `Literal`
* `Ident`
* `EnumVariant(name, args...)`
* `Struct(name, fields...)`

---

## Type system (current)

### Base types

* `Int`, `Float`, `Bool`, `String`, `Bytes`, `Html`
* `Id` (non-empty string)
* `Email` (string validated by a simple `user@host` check)
* `Error` (built-in error base)
* `List<T>`, `Map<K,V>`, `Option<T>`, `Result<T,E>`
* Runtime constraint: `Map<K,V>` requires `K = String`.
* Runtime detail: `Bytes` values are raw bytes; JSON/config/CLI boundaries use base64 text.
* Runtime detail: `Html` values are runtime HTML trees built via `html.*` builtins.
* User-defined `type` and `enum` are nominal.

Reserved namespace:

* `std.Error.*` is reserved for runtime error mapping/JSON rendering.

### Optionals (`T?`)

* `T?` is `Option<T>`.
* `null` represents `None`.
* `x ?? y` returns `x` unless it is `null`, otherwise `y`.
* `x?.field` returns `null` if `x` is `null`, otherwise the field value.
* `Some` / `None` are valid match patterns.

### Results (`T!` / `T!E`)

* `T!` means `Result<T, Error>`.
* `T!E` means `Result<T, E>`.
* `expr ?! err` converts `Option`/`Result` to a typed error.
* `expr ?!` uses a default error for `Option` and propagates the existing error for `Result`.

### Refined types

Refinements attach range predicates to a base type:

* `String(1..80)` length constraint
* `Int(0..130)` numeric range
* `Float(0.0..1.0)` numeric range

Runtime expects range literals or a `..` expression inside the refinement. Other refinements
(like regex) are not implemented yet.

### Type inference

* Local inference for `let`/`var`.
* Function parameter types are required.
* Function return type is optional.

### Structural vs nominal

* User-defined `type` and `enum` are nominal.
* Anonymous record types do not exist in the current grammar.

### Type derivations (`without`)

`type PublicUser = User without password, secret` creates a new nominal type by copying
fields from the base `type` and removing the listed fields. Field types and defaults are
preserved. Base types can be module-qualified (`Foo.User`). Unknown base types or fields
are errors.

---

## Imports and modules (current)

`import` declarations are resolved at load time. Module imports register a module alias for
qualified access (`Foo.bar`, `Foo.Config.field`, `Foo.Enum.Variant`). Named imports bring specific
items into the local scope.

Resolution rules:

* `import Foo` loads `Foo.fuse` from the current file's directory.
* `import X from "path"` loads `path` relative to the current file; `.fuse` is added if missing.
* `import {A, B} from "path"` loads the module and brings only `A` and `B` into scope.

Notes:

* Module imports do not add names to the local scope; use `Foo.bar` for access.
* Named imports (`import {A, B} from "path"`) do not create a module alias.
* Module-qualified access only exposes items declared in that module (named imports are local).
* Module-qualified type references are allowed in type positions (`Foo.User`, `Foo.Config`).
* Names are still globally unique across loaded modules.
* Dependency modules are imported with `dep:` prefixes (for example, `dep:Auth/lib`).

---

## Services and binding (summary)

Route syntax uses typed path params inside the string literal, for example:
`get "/users/{id: Id}" -> User: ...`

The `body` keyword introduces the request body type:
`post "/users" body UserCreate -> User: ...`

Runtime binding + error mapping are described in `runtime.md`.

---

## Runtime support notes (current)

* `migration` blocks run via `fusec --migrate` (AST backend only).
* `test` blocks run via `fusec --test` (AST backend only).
* `for`/`while`/`break`/`continue` run in AST, VM, and native backends.
* `spawn`/`await` run in AST, VM, and native backends (tasks execute eagerly today).
* `Task<T>` values are opaque runtime values with a minimal task API.
* Minimal task API is available: `task.id`, `task.done`, and `task.cancel`.
* With eager execution today, tasks are usually already done and `task.cancel` usually returns `false`.
* `box` creates a shared cell; boxed values are transparently dereferenced in most expressions.
* Assignment targets include identifiers, struct fields, and list/map indexing; optional access in assignments errors on null.
* Enum variants only support tuple payloads (no named payload fields).
* `a..b` yields a numeric list (inclusive) when evaluated; descending ranges error.
* DB builtins support parameter binding and a minimal query builder (`db.from` / `query.*`).
* HTML builtins are available (`html.text`, `html.raw`, `html.node`, `html.render`).
* HTTP handlers returning `Html` are rendered with `Content-Type: text/html; charset=utf-8`.
* `native` currently reuses VM runtime semantics while compiler/codegen work is still in progress, with a Cranelift JIT fast-path for direct Int/Bool arithmetic/control-flow function calls.

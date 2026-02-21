# Formal language specification (parser + semantic analysis)

This document tracks the parser and semantic analysis implemented in `crates/fusec`.
It is the canonical source for lexical rules, grammar, AST structure, and static type semantics.

Runtime behavior (validation timing, HTTP status mapping, config/CLI parsing, DB/task execution)
is intentionally documented in `runtime.md`.

---

## Scope of this document

Covered here:

- lexing and indentation rules
- grammar and parse-level sugar
- AST model shape
- static type model and module/import resolution

Not covered here:

- runtime value encoding/decoding behavior
- runtime boundary binding rules
- backend execution details and builtins behavior

See also: [Lexing + indentation rules](#lexing--indentation-rules), [Runtime semantics](runtime.md).

---

## Lexing + indentation rules

### Tokens

- Identifiers: `[A-Za-z_][A-Za-z0-9_]*`
- Keywords:
  `app, service, at, get, post, put, patch, delete, fn, type, enum, let, var, return, if, else,
  match, for, in, while, break, continue, import, from, as, config, migration, test,
  body, and, or, without, spawn, await, box`
- Literals:
  - integers (`123`)
  - floats (`3.14`)
  - strings (`"hello"`)
  - booleans (`true`, `false`)
  - `null`
- Punctuation/operators:
  `(` `)` `[` `]` `{` `}` `,` `:` `.` `..` `->` `=>` `=` `==` `!=` `<` `<=` `>` `>=`
  `+` `-` `*` `/` `%` `?` `!` `??` `?!`

### Strings + interpolation

- Double-quoted strings only (no multiline strings).
- Escapes: `\n`, `\t`, `\r`, `\\`, `\"`. Unknown escapes pass through (`\$` produces `$`).
- Interpolation: `${expr}` inside double quotes.

### Significant indentation

FUSE uses Python-style block structure with strict space rules.

- Indentation is measured in spaces only (tabs are illegal).
- Indent width is not fixed, but must be consistent within a file.
- A block starts after `:` at end of line.
- New indentation level must be strictly greater than previous.
- Dedent closes blocks until indentation matches a previous level.
- Empty lines are ignored.
- Lines inside parentheses/brackets/braces ignore indentation semantics (implicit line joining).

INDENT/DEDENT algorithm (formal-ish):

- Maintain a stack `indents` starting with `[0]`.
- For each logical line not inside `()[]{}`:
  - Let `col` be count of leading spaces.
  - If `col > top(indents)`: emit `INDENT`, push `col`.
  - If `col < top(indents)`: while `col < top(indents)` emit `DEDENT`, pop;
    error if `col != top(indents)` after popping.
  - Else: continue.

### Comments

- Line comment: `# ...`
- Doc comment: `## ...` attaches to the next declaration

See also: [Grammar (EBNF-ish)](#grammar-ebnf-ish), [AST model (structural spec)](#ast-model-structural-spec).

---

## Grammar (EBNF-ish)

Top level:

```ebnf
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
Pattern        := "_" | Literal | Ident [ "(" PatternArgs ")" ]
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

See also: [AST model (structural spec)](#ast-model-structural-spec), [Type system (current static model)](#type-system-current-static-model), [Runtime semantics](runtime.md).

---

## AST model (structural spec)

The AST shape matches `crates/fusec/src/ast.rs`.

Program:

- `Program { items: Vec<Item> }`

Items:

- `Import(ImportDecl)`
- `Type(TypeDecl)`
- `Enum(EnumDecl)`
- `Fn(FnDecl)`
- `Service(ServiceDecl)`
- `Config(ConfigDecl)`
- `App(AppDecl)`
- `Migration(MigrationDecl)`
- `Test(TestDecl)`

Declarations:

- `TypeDecl { name, fields, derive, doc }`
- `TypeDerive { base, without }`
- `FieldDecl { name, ty, default }`
- `EnumDecl { name, variants, doc }`
- `EnumVariant { name, payload }`
- `FnDecl { name, params, ret, body, doc }`
- `ServiceDecl { name, base_path, routes, doc }`
- `RouteDecl { verb, path, body_type, ret_type, body }`
- `ConfigDecl { name, fields, doc }`
- `ConfigField { name, ty, value }`
- `AppDecl { name, body, doc }`
- `MigrationDecl { header, body, doc }`
- `TestDecl { name, body, doc }`

Statements:

- `Let { name, ty, expr }`
- `Var { name, ty, expr }`
- `Assign { target, expr }`
- `Return { expr }`
- `If { cond, then_block, else_if, else_block }`
- `Match { expr, cases }`
- `For { pat, iter, block }`
- `While { cond, block }`
- `Expr(expr)`
- `Break`
- `Continue`

Expressions:

- `Literal`
- `Ident`
- `Binary(op, left, right)`
- `Unary(op, expr)`
- `Call(callee, args)` where args are `CallArg { name, value, is_block_sugar }`
- `Member(base, name)`
- `OptionalMember(base, name)`
- `Index(base, index)`
- `OptionalIndex(base, index)`
- `StructLit(name, fields)`
- `ListLit(items)`
- `MapLit(pairs)`
- `InterpString(parts)`
- `Coalesce(left, right)`
- `BangChain(expr, error?)`
- `Spawn(block)`
- `Await(expr)`
- `Box(expr)`

Patterns:

- `Wildcard`
- `Literal`
- `Ident`
- `EnumVariant(name, args...)`
- `Struct(name, fields...)`

See also: [Grammar (EBNF-ish)](#grammar-ebnf-ish), [Type system (current static model)](#type-system-current-static-model).

---

## Type system (current static model)

### Base types

- `Int`, `Float`, `Bool`, `String`, `Bytes`, `Html`
- `Id`, `Email`
- `Error`
- `List<T>`, `Map<K,V>`, `Option<T>`, `Result<T,E>`
- user-defined `type` and `enum` are nominal

Reserved namespace:

- `std.Error.*` is reserved for standardized runtime error behavior.

### Optionals (`T?`)

- `T?` desugars to `Option<T>`.
- `null` is the optional empty value.
- `x ?? y` is null-coalescing.
- `x?.field` and `x?[idx]` are optional access forms.
- `Some` / `None` are valid match patterns.

### Results (`T!` / `T!E`)

- `T!` desugars to `Result<T, Error>`.
- `T!E` desugars to `Result<T, E>`.
- `expr ?! err` applies bang-chain error conversion.
- `expr ?!` uses default/propagated error behavior (runtime details in `runtime.md`).

### Refined types

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

### Structural vs nominal

- user-defined `type` and `enum` are nominal
- anonymous record types are not part of the current grammar

### Type derivations (`without`)

`type PublicUser = User without password, secret` creates a new nominal type derived from `User`
with listed fields removed. Field types/defaults are preserved for retained fields.

Base types can be module-qualified (`Foo.User`). Unknown base types or fields are errors.

See also: [Imports and modules (current)](#imports-and-modules-current), [Runtime semantics](runtime.md), [Scope + constraints](scope.md).

---

## Imports and modules (current)

`import` declarations are resolved at load time.

- Module imports register an alias for qualified access (`Foo.bar`, `Foo.Config.field`, `Foo.Enum.Variant`).
- Named imports bring specific items into local scope.

Resolution rules:

- `import Foo` loads `Foo.fuse` from the current file directory.
- `import X from "path"` loads `path` relative to current file; `.fuse` is added if missing.
- `import {A, B} from "path"` loads module and imports listed names into local scope.
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

See also: [Services and declaration syntax](#services-and-declaration-syntax), [FUSE overview](fuse.md).

---

## Services and declaration syntax

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

See also: [Runtime semantics](runtime.md), [Error model](runtime.md#error-model), [Boundary model](runtime.md#boundary-model).

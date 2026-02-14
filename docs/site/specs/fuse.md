# Start Here: Your First FUSE Service

This guide gets you from zero to a running FUSE HTTP service.

---

## Step 1: Create a package

Create `fuse.toml`:

```toml
[package]
entry = "src/main.fuse"
app = "Api"
backend = "vm"
```

Create `src/main.fuse`:

```fuse
config App:
  port: Int = env("PORT") ?? 3000

type UserCreate:
  email: Email
  name: String(1..80)

service Api at "/api":
  post "/users" body UserCreate -> UserCreate:
    return body

app "Api":
  serve(App.port)
```

Run it:

```bash
fuse run .
```

Next step: if this is your first FUSE file, continue to [Language Tour](fls.md) to learn the syntax you just used.

---

## Step 2: Understand what just happened

In one file, you defined:

- config loading (`config App`)
- validated request schema (`type UserCreate`)
- typed route contract (`post ... body UserCreate -> UserCreate`)
- executable app entry (`app "Api"`)

FUSE uses the same types for validation, parsing, and response encoding.

---

## Step 3: Add typed error handling

```fuse
type std.Error.NotFound:
  message: String

fn load_user(id: Id) -> User!std.Error.NotFound:
  let user = find_user(id) ?! std.Error.NotFound(message="User not found")
  return user
```

Use `T?`, `T!`, and `?!` to keep failure paths explicit and typed.

---

## Step 4: Daily workflow

During development:

- `fuse check` for semantic checks
- `fuse dev` for watch + live reload
- `fuse test` for `test` blocks
- `fuse build` for build artifacts (including OpenAPI when configured)

If you are ready to run and debug real services, go to [Build + Operate](runtime.md). If you are evaluating fit and tradeoffs, continue with [Limits + Roadmap](scope.md).

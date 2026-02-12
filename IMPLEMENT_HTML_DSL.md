# PHASE 0 — Non-Negotiable Principles

Status: DONE

Before adding anything:

1. **No grammar expansion except HTML block DSL.**
2. **No client runtime added to language.**
3. **Everything is optional and opt-in.**
4. **Production build must remain minimal.**
5. **Dev tooling must not leak into runtime semantics.**

If any feature violates these, it doesn’t ship.

---

# PHASE 1 — HTML Tree (Core Runtime)

Status: DONE

## 1.1 New Type

```fuse
type Html
```

Runtime representation (Rust side):

```rust
enum HtmlNode {
    Element { tag, attrs, children },
    Text(String),
    Raw(String),
}
```

## 1.2 Builtins

```fuse
html.text(String) -> Html
html.raw(String) -> Html
html.node(name: String, attrs: Map<String,String>, children: List<Html>) -> Html
html.render(Html) -> String
```

HTTP layer update:

* If route return type is `Html`
* Automatically set `Content-Type: text/html`
* Render once, no streaming yet

No template language.
No implicit escaping.
No string auto-coercion.

---

# PHASE 2 — Block DSL (Scoped)

Status: DONE

Syntax allowed:

```fuse
div():
  h1():
    text("Hello")
```

## 2.1 Compiler Lowering

This:

```fuse
div():
  h1():
    text("Hello")
```

Lowers to:

```fuse
div({}, [
  h1({}, [
    text("Hello")
  ])
])
```

## 2.2 Strict Rules

* Only functions returning `Html` may use block form.
* Block implicitly returns `List<Html>`.
* No implicit string children.
* No attribute shorthand.
* No arbitrary block-as-argument for other functions.

This keeps DSL domain-limited.

---

# PHASE 3 — Dev Server + Live Reload

Status: DONE

Command:

```bash
fuse dev
```

Behavior:

* Watches `.fuse`
* Watches SCSS (if configured)
* Restarts backend
* Injects minimal reload script

Reload implementation:

* WebSocket endpoint `/__reload`
* Browser script auto-reconnects and refreshes

No HMR.
No state preservation.
Simple page reload.

---

# PHASE 4 — OpenAPI UI Auto Serve

Status: DONE

Implemented behavior in this repo:

```toml
[serve]
openapi_ui = true
openapi_path = "/docs"
```

`fuse dev` enables OpenAPI UI by default; `fuse run` requires explicit opt-in (`openapi_ui = true`).

Equivalent app-level intent:

```fuse
app "Api":
  serve(App.port)
  expose openapi_ui at "/docs"
```

Implementation:

* Embed static Swagger UI bundle
* Serve generated OpenAPI JSON
* No runtime spec generation

Production:

* disabled unless opt-in

---

# PHASE 5 — HTMX-Friendly Patterns

Status: DONE

No HTMX runtime integration.
Just patterns:

* Return `Html` fragments.
* Support status codes normally.
* Encourage server-driven swaps.

Example:

```fuse
post "/notes" body Note -> Html:
  return note_row(note)
```

That’s it.

No reactive model.

---

# PHASE 6 — Asset Pipeline (Minimal & Honest)

Status: DONE

This is orchestration, not bundling.

## 6.1 fuse.toml

```toml
[assets]
scss = "assets/scss"
css = "public/css"
watch = true
hash = true
```

## 6.2 Behavior

Dev:

* Watch SCSS
* Run external `sass`
* Serve compiled CSS

Build:

* Compile SCSS

No internal SCSS parser.

Current implementation note: SCSS orchestration is implemented via external `sass`
for `fuse build` and `fuse dev`. Hashing + `asset(...)` landed in PHASE 7.

---

# PHASE 7 — Hashed Static Files

Status: DONE

When `hash = true`:

* Compute content hash
* Rename:

  ```
  app.css → app.3f92a.css
  ```
* Provide helper:

```fuse
asset("css/app.css") -> String
```

So in HTML:

```fuse
link({ href: asset("css/app.css") })
```

This avoids manual rewriting.

Implemented behavior in this repo:

* `fuse build`/`fuse dev` with `[assets].hash = true` renames compiled CSS to content-hashed filenames.
* Hashes are exposed to runtime via `.fuse/assets-manifest.json` and `FUSE_ASSET_MAP`.
* New builtin helper `asset(path: String) -> String` resolves logical paths (for example `css/app.css`)
  to hashed public URLs (for example `/css/app.<hash>.css`), with fallback to `/<path>`.

---

# PHASE 8 — Asset Hooks

Status: PENDING

Allow external tool integration.

In `fuse.toml`:

```toml
[assets.hooks]
before_build = "npm run build"
```

FUSE runs hook before production build.

This enables:

* Vite
* esbuild
* any custom pipeline

Without FUSE owning JS ecosystem.

---

# PHASE 9 — Vite Integration

Status: PENDING

If project uses Vite:

* Dev: proxy unknown routes to Vite dev server
* Production: serve `dist/` output

FUSE acts as backend + proxy, not bundler.

---

# PHASE 10 — Optional SVG Loading

Status: PENDING

Add server-side inline SVG support with deterministic resolution.

## API:

```
  svg.inline(name: String) -> Html
```

## Resolution Rules

- Base directory is fixed: /assets/svg/
- If extension is omitted → append ".svg"
- Allow nested subfolders (e.g. "icons/ui/close")
- Reject any ".." path traversal
- No fallback chains, no dynamic resolution
- Fail loudly if file does not exist

## Runtime Behavior

- Dev: read from disk on change
- Production: preload and cache at startup
- No parsing or transformation
- No DOM manipulation
- No optimization or minification
- Return as html.raw(contents)

## Purpose

- Enable inline SVG for styling and reuse
- Keep resolution predictable and boring
- Avoid building a resolver or asset framework

---

# Final Architecture

```
FUSE Core:
  - Types
  - HTTP
  - DB
  - Html tree
  - JSON
  - CLI

Dev Layer:
  - Live reload
  - SCSS coordination
  - OpenAPI UI
  - Asset watcher

Optional:
  - Vite proxy
  - SVG inline
```

---

# What This Does NOT Become

* Not React.
* Not Next.js.
* Not Laravel.
* Not Rails.
* Not Phoenix.
* Not a bundler.

It becomes:

> A deterministic server-rendered full-stack backend with ergonomic dev tooling.

---

# Strategic Positioning

This would make FUSE:

* Strong for small apps
* Strong for dashboards
* Strong for internal tools
* Strong for HTMX-style apps
* Strong for API-first projects

Without diluting its core philosophy.

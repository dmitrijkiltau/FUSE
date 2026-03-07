#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_DIR="$ROOT/guides/src"
OUT_DIR="$ROOT/guides"

if [[ "${FUSE_SKIP_GUIDE_DOCS:-0}" == "1" ]]; then
  echo "skipping guide docs generation (FUSE_SKIP_GUIDE_DOCS=1)"
  exit 0
fi

if [[ ! -d "$SRC_DIR" ]]; then
  echo "guide source directory not found: $SRC_DIR" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"

# ---------------------------------------------------------------------------
# extract_section FILE HEADING [STOP_HEADING]
#
# Prints lines from FILE starting after the line matching HEADING (a markdown
# heading like "## Foo") up to (but not including) either STOP_HEADING or the
# next heading of equal or higher level.  Leading/trailing blank lines and
# "See also:" lines at the tail are trimmed.
# ---------------------------------------------------------------------------
extract_section() {
  local file="$1" heading="$2" stop="${3:-}"
  local level
  level="$(echo "$heading" | sed 's/^\(#*\).*/\1/' | wc -c)"
  # level is length + 1 due to wc trailing newline, so adjust
  level=$((level - 1))

  awk -v heading="$heading" -v stop="$stop" -v level="$level" '
  BEGIN { found = 0 }
  {
    if (!found) {
      if ($0 == heading) { found = 1 }
      next
    }
    # stop at explicit stop heading
    if (stop != "" && $0 == stop) exit
    # stop at any heading of equal or higher level
    if (/^#{1,6} / ) {
      n = 0
      s = $0
      while (substr(s, n+1, 1) == "#") n++
      if (n <= level) exit
    }
    print
  }
  ' "$file" | sed -e '/^See also:/d' -e '/^[[:space:]]*---[[:space:]]*$/d' \
    | sed -e :a -e '/^[[:space:]]*$/{ $d; N; ba; }' \
    | sed '/./,$!d'
}

# ---------------------------------------------------------------------------
# generate_reference
#
# Assembles guides/reference.md from spec/fls.md, spec/runtime.md, and governance/scope.md.
# This is the canonical GitHub-facing reference surface.
# ---------------------------------------------------------------------------
generate_reference() {
  local dst="$OUT_DIR/reference.md"
  local fls="$ROOT/spec/fls.md"
  local rtm="$ROOT/spec/runtime.md"
  local scope="$ROOT/governance/scope.md"

  {
    cat <<'HEADER'
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
- `component`
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
- `transaction`
- `break` / `continue`
- `return`

Core expression features:

- null-coalescing: `??`
- optional access: `?.`, `?[idx]`
- bang-chain conversion: `?!`
- ranges: `a..b`
- concurrency forms: `spawn`, `await`, `box`

---
HEADER

    # --- Types (from spec/fls.md) ---
    echo "## Types"
    echo
    extract_section "$fls" "### Base types"
    echo
    echo "Type shorthand:"
    echo
    extract_section "$fls" '### Optionals (`T?`)' '### Results (`T!` / `T!E`)'
    echo
    echo "Result types:"
    echo
    extract_section "$fls" '### Results (`T!` / `T!E`)' '### Refined types'
    echo
    echo "Refinements:"
    echo
    extract_section "$fls" "### Refined types" "### Type inference"
    echo
    echo "### Type inference"
    echo
    extract_section "$fls" "### Type inference" "### Comparison operators"
    echo
    echo "### Comparison operators"
    echo
    extract_section "$fls" "### Comparison operators" "### Structural vs nominal"
    echo
    echo "Type derivation:"
    echo
    extract_section "$fls" '### Type derivations (`without`)'
    echo
    echo "---"
    echo

    # --- Lexing and Strings (from spec/fls.md) ---
    echo "## Strings, Interpolation, and Comments"
    echo
    extract_section "$fls" "### Strings + interpolation" "### Significant indentation"
    echo
    extract_section "$fls" "### Comments"
    echo
    echo "## Indentation"
    echo
    extract_section "$fls" "### Significant indentation"
    echo
    echo "---"
    echo

    # --- Match and Patterns ---
    cat <<'MATCH_PATTERNS'
## Match and Patterns

`match` executes the first case whose pattern matches the value.

Case forms:

- `Pattern -> Expr` is a single-expression case (sugar for `Pattern: return Expr`).
- `Pattern:` followed by an indented block is the full block form.

Pattern forms:

- `_` — wildcard, matches any value
- `Literal` — integer, float, string, or bool literal
- `None` — matches optional empty value
- `Some(x)` — matches optional present value, binds the payload to `x`
- `Ok(x)` / `Err(e)` — matches result variants, binds the payload
- `EnumVariant` — matches a no-payload enum variant by name
- `EnumVariant(x, y)` — matches an enum variant with positional payload bindings
- `TypeName(field = pattern, ...)` — matches struct fields by name

---
MATCH_PATTERNS

    # --- Imports and Modules (from spec/fls.md) ---
    echo "## Imports and Modules"
    echo
    extract_section "$fls" "## Imports and modules (current)"
    echo
    echo "---"
    echo

    # --- Services (from spec/fls.md) ---
    echo "## Services and HTTP Contracts"
    echo
    extract_section "$fls" "## Services and declaration syntax"
    echo
    echo "---"
    echo

    # --- Spawn and Transaction Restrictions (from spec/fls.md) ---
    echo "## Static Restrictions"
    echo
    echo "### Spawn static restrictions"
    echo
    extract_section "$fls" "### Spawn static restrictions" "### Transaction static restrictions"
    echo
    echo "### Transaction static restrictions"
    echo
    extract_section "$fls" "### Transaction static restrictions"
    echo
    echo "---"
    echo

    # --- Runtime Behavior (from spec/runtime.md) ---
    echo "## Runtime Behavior"
    echo
    echo "### Expression operator behavior"
    echo
    extract_section "$rtm" "## Expression operator behavior" "## Error model"
    echo
    echo "### Validation and boundary enforcement"
    echo
    extract_section "$rtm" "### Validation" "### JSON encoding/decoding"
    echo
    echo "### JSON behavior"
    echo
    extract_section "$rtm" "### JSON encoding/decoding" "### Config loading"
    echo
    echo "### Errors and HTTP status mapping"
    echo
    extract_section "$rtm" "### Recognized error names" "### Error JSON shape"
    echo
    echo "### Error JSON shape"
    echo
    extract_section "$rtm" "### Error JSON shape" "### HTTP status mapping"
    echo
    extract_section "$rtm" "### HTTP status mapping" '### Result types + `?!`'
    echo
    echo '`expr ?! err` behavior:'
    echo
    extract_section "$rtm" '### Result types + `?!`'
    echo
    echo "### Config and CLI binding"
    echo
    extract_section "$rtm" "### Config loading" "### CLI binding"
    echo
    echo "CLI binding:"
    echo
    extract_section "$rtm" "### CLI binding" "### HTTP runtime"
    echo
    echo "---"
    echo

    # --- HTTP Runtime (from spec/runtime.md) ---
    echo "## HTTP Runtime"
    echo
    echo "### Routing"
    echo
    extract_section "$rtm" "#### Routing" "#### Response"
    echo
    echo "### Response"
    echo
    extract_section "$rtm" "#### Response" "#### Request primitives"
    echo
    echo "### Request primitives"
    echo
    extract_section "$rtm" "#### Request primitives" "#### Environment knobs"
    echo
    echo "### Observability baseline"
    echo
    extract_section "$rtm" "#### Observability baseline"
    echo
    echo "---"
    echo

    # --- Builtins (from spec/runtime.md) ---
    echo "## Builtins"
    echo
    extract_section "$rtm" "### Builtins (current)" "### Compile-time capability requirements"
    echo
    echo "---"
    echo

    # --- Database (from spec/runtime.md) ---
    echo "## Database (SQLite)"
    echo
    extract_section "$rtm" "### Database (SQLite only)" "### Migrations"
    echo
    echo "### Migrations"
    echo
    extract_section "$rtm" "### Migrations" "### Tests"
    echo
    echo "### Tests"
    echo
    extract_section "$rtm" "### Tests" "### Concurrency"
    echo
    echo "---"
    echo

    # --- Concurrency (from spec/runtime.md) ---
    echo "## Concurrency"
    echo
    extract_section "$rtm" "### Concurrency" "### Loops"
    echo
    echo "---"
    echo

    # --- Loops, Indexing, Ranges (from spec/runtime.md) ---
    echo "## Loops, Indexing, and Ranges"
    echo
    echo "### Loops"
    echo
    extract_section "$rtm" "### Loops" "### Indexing"
    echo
    echo "### Indexing"
    echo
    extract_section "$rtm" "### Indexing" "### Ranges"
    echo
    echo "### Ranges"
    echo
    extract_section "$rtm" "### Ranges" "### Logging"
    echo
    echo "---"
    echo

    # --- Logging (from spec/runtime.md) ---
    echo "## Logging"
    echo
    extract_section "$rtm" "### Logging"
    echo
    echo "---"
    echo

    # --- Tooling ---
    cat <<'TOOLING'
## Tooling and Package Commands

Common package commands:

- `fuse check` — parse and semantic-check a package
- `fuse run` — run a package
- `fuse dev` — run in watch/dev mode with live reload
- `fuse test` — run test blocks
- `fuse build` — compile to a native binary
- `fuse clean --cache` — remove `.fuse-cache` directories under a selected root
- `fuse fmt` — format a source file
- `fuse openapi` — emit an OpenAPI JSON document
- `fuse migrate` — execute pending migration blocks
- `fuse lsp` — start the language server

Useful flags:

- `fuse build --clean` — remove `.fuse/build` before building
- `--workspace` — check all packages under the current directory
- `--strict-architecture` — enable architectural purity checks
- `--diagnostics json` — emit diagnostics as JSON Lines on stderr

`fuse.toml` manifest sections:

| Section | Purpose |
|---|---|
| `[package]` | Entry source file (`entry`), app/service name (`app`), runtime backend (`backend`) |
| `[build]` | Build outputs: `native_bin` binary path, `openapi` JSON output path |
| `[serve]` | Server defaults: `static_dir` for static file serving |
| `[assets]` | Named asset entries (CSS, JS) and `watch` flag |
| `[assets.hooks]` | Build hooks for asset processing |
| `[vite]` | Vite dev server integration settings |
| `[dependencies]` | Package dependencies for `dep:` import paths |

---
TOOLING

    # --- Environment Variables ---
    cat <<'ENVTABLE'

## Runtime Environment Variables

| Variable | Default | Description |
|---|---|---|
| `FUSE_DB_URL` | — | Database connection URL (`sqlite://path`) |
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
| `FUSE_DEV_RELOAD_WS_URL` | — | Enables dev HTML script injection (`/__reload` client) with reload + compile-error overlay events |
| `FUSE_OPENAPI_JSON_PATH` | — | Enables built-in OpenAPI JSON endpoint at this path |
| `FUSE_OPENAPI_UI_PATH` | — | Enables built-in OpenAPI UI at this path |
| `FUSE_ASSET_MAP` | — | Logical-path to public-URL mappings for `asset(path)` |
| `FUSE_VITE_PROXY_URL` | — | Fallback proxy for unknown routes to Vite dev server |
| `FUSE_SVG_DIR` | — | Override SVG base directory for `svg.inline` |
| `FUSE_STATIC_DIR` | — | Serve static files from this directory |
| `FUSE_STATIC_INDEX` | `index.html` | Fallback file for directory requests when `FUSE_STATIC_DIR` is set |

### AOT binary environment variables

The following variables are only effective in compiled AOT binaries (`fuse build --release`):

| Variable | Description |
|---|---|
| `FUSE_AOT_BUILD_INFO` | Print AOT build metadata and exit |
| `FUSE_AOT_STARTUP_TRACE` | Emit a startup diagnostic line to stderr |
| `FUSE_AOT_REQUEST_LOG_DEFAULT` | Default to structured request logging when `FUSE_REQUEST_LOG` is unset |

ENVTABLE
    echo "---"
    echo

    # --- Constraints (from governance/scope.md) ---
    echo "## Constraints"
    echo
    echo "Current practical constraints:"
    echo
    echo "- SQLite-focused database runtime"
    echo "- no full ORM layer"
    echo "- task model is still evolving"

  } >"$dst"

  echo "generated: ${dst#$ROOT/}"
}

render_markdown_from_source() {
  local src="$1"
  awk '
  BEGIN {
    in_code = 0
  }
  /^# @title[[:space:]]+/ {
    next
  }
  /^# @summary[[:space:]]+/ {
    next
  }
  {
    line = $0
    if (line ~ /^#/) {
      sub(/^# ?/, "", line)
      if (in_code) {
        print "```"
        print ""
        in_code = 0
      }
      print line
      next
    }

    if (line ~ /^[[:space:]]*$/) {
      if (in_code) {
        print line
      } else {
        print ""
      }
      next
    }

    if (!in_code) {
      print "```fuse"
      in_code = 1
    }
    print $0
  }
  END {
    if (in_code) {
      print "```"
    }
  }
  ' "$src"
}

count=0
for src in "$SRC_DIR"/*.fuse; do
  [[ -e "$src" ]] || continue

  slug="$(basename "$src" .fuse)"
  dst="$OUT_DIR/$slug.md"
  rel_src="${src#$ROOT/}"

  title="$(sed -n 's/^# @title[[:space:]]*//p' "$src" | head -n 1)"
  if [[ -z "$title" ]]; then
    title="$(basename "$src" .fuse)"
  fi

  {
    echo "# $title"
    echo
    while IFS= read -r summary; do
      [[ -n "$summary" ]] || continue
      echo "$summary"
    done < <(sed -n 's/^# @summary[[:space:]]*//p' "$src")
    echo
    echo "_Generated from \`$rel_src\` by \`scripts/generate_guide_docs.sh\`._"
    echo
    render_markdown_from_source "$src"
    echo
  } >"$dst"

  count=$((count + 1))
  echo "generated: ${dst#$ROOT/}"
done

if [[ "$count" -eq 0 ]]; then
  echo "no guide sources found in $SRC_DIR" >&2
  exit 1
fi

# Generate reference.md from spec files
generate_reference

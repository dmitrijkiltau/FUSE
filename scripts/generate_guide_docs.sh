#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_DIR="$ROOT/docs/src/guides"
OUT_DIR="$ROOT/docs/site/specs"

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
# Assembles docs/site/specs/reference.md from spec/fls.md, spec/runtime.md, and governance/scope.md.
# This replaces the previously hand-maintained reference.md.
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
    echo "Type derivation:"
    echo
    extract_section "$fls" '### Type derivations (`without`)'
    echo
    echo "---"
    echo

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

    # --- Runtime Behavior (from spec/runtime.md) ---
    echo "## Runtime Behavior"
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
    extract_section "$rtm" "### Recognized error names" '### Result types + `?!`'
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

    # --- Builtins (from spec/runtime.md) ---
    echo "## Builtins"
    echo
    extract_section "$rtm" "### Builtins (current)" "### Database (SQLite only)"
    echo
    echo "Database builtins:"
    echo
    echo "- \`db.exec\`, \`db.query\`, \`db.one\`"
    echo "- \`db.from\` + query builder methods"
    echo
    echo "Current DB mode is SQLite-focused."
    echo
    echo "---"
    echo

    # --- Tooling ---
    cat <<'TOOLING'
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
TOOLING

    # --- Docker ---
    cat <<'DOCKER'

## Run Docs with Docker

`docs/Dockerfile` builds the `fuse` CLI from source, then runs `fuse build --release` to produce the docs AOT binary.
Guide docs are not generated by package build/run hooks; committed generated docs are used in Docker.
Downloadable release artifacts are not served by the docs app; use GitHub Releases instead.

Build the docs image from repository root:

```bash
docker build -f docs/Dockerfile -t fuse-docs:0.7.0 .
```

Run the docs container:

```bash
docker run --rm -p 4080:4080 -e PORT=4080 -e FUSE_HOST=0.0.0.0 fuse-docs:0.7.0
```

Then open <http://localhost:4080>.

You can also use Compose:

```bash
docker compose --project-directory . -f docs/docker-compose.yml up --build
```

---
DOCKER

    # --- Environment Variables (from spec/runtime.md) ---
    echo
    echo "## Runtime Environment Variables"
    echo
    extract_section "$rtm" "#### Environment knobs"
    echo
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
    echo "- native backend uses Cranelift JIT"

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

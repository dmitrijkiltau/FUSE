#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_DIR="$ROOT/docs/src/guides"
OUT_DIR="$ROOT/docs/site/specs"

if [[ ! -d "$SRC_DIR" ]]; then
  echo "guide source directory not found: $SRC_DIR" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"

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

#!/usr/bin/env bash

fuse_repo_root() {
  local script_path="$1"
  (cd "$(dirname "$script_path")/.." && pwd)
}

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

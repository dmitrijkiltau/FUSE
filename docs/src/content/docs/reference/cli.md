---
title: CLI reference
description: fusec developer tooling.
---

## `fusec` flags

- `--check`: parse + semantic analysis
- `--run`: execute the program
- `--backend ast|vm`: choose execution backend
- `--app NAME`: select an app entry
- `--fmt`: format a file

## Wrapper script

Use `scripts/fuse` to run with safe cargo env settings:

```
scripts/fuse check <file>
scripts/fuse run <file>
scripts/fuse fmt <file>
```

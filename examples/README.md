# Examples

Sample programs for FUSE. All examples are valid FUSE source files that pass
`fuse check`. Many can also be executed with `fuse run`.

## Running examples

```bash
# Type-check a file
./scripts/fuse check examples/cli_hello.fuse

# Run with the default backend
./scripts/fuse run examples/cli_hello.fuse

# Run with a specific backend
./scripts/fuse run --backend native examples/project_demo.fuse

# Run with environment overrides
APP_PORT=4000 ./scripts/fuse run examples/http_users.fuse

# Check all examples at once
./scripts/check_examples.sh
```

## Language examples

| File | Topic |
|---|---|
| `cli_hello.fuse` | CLI hello with config defaults |
| `cli_args.fuse` | CLI args binding (flags and values) |
| `cli_input.fuse` | CLI stdin input with prompt |
| `log_parity.fuse` | Runtime log text/JSON output behavior |
| `http_users.fuse` | HTTP service with routes and `?!` error handling |
| `types_patterns.fuse` | Enums, structs, and pattern matching (Option/Result) |
| `project_demo.fuse` | Config env overrides, refined types, enums, and match |
| `interp_demo.fuse` | String interpolation |
| `spawn_error.fuse` | Spawn/await task failure propagation |
| `box_shared.fuse` | Shared `box` state mutation |
| `assign_field.fuse` | Struct field assignment |
| `assign_index.fuse` | List/map index assignment |
| `range_demo.fuse` | Range expressions (inclusive lists) |
| `enum_match.fuse` | Enum declarations and match expressions |
| `float_compare.fuse` | Float comparison semantics |
| `task_api.fuse` | Spawn/await task workflow |
| `db_query_builder.fuse` | DB query-builder workflow (`db.from`) |

## Native backend examples

These exercise the Cranelift JIT backend specifically (`--backend native`):

| File | Topic |
|---|---|
| `native_bang_error.fuse` | `?!` error handling |
| `native_bench.fuse` | Performance smoke test |
| `native_builtins.fuse` | Builtin coverage |
| `native_db.fuse` | Database execution |
| `native_heap_literals.fuse` | Heap-allocated literals |
| `native_json.fuse` | JSON encode/decode |
| `native_validation.fuse` | Validation behavior |

## Package example

The `reference-service/` directory is the canonical package-level service example.
It includes `fuse.toml`, HTML templates, static assets, and SCSS compilation.
See [reference-service/README.md](reference-service/README.md).

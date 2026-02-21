# Examples

These examples are meant to be parsed and sema-checked by `fusec`. Some can also be run with the interpreter via `--run`.

Check an example:

```
scripts/cargo_env.sh cargo run -p fusec -- --check examples/cli_hello.fuse
```

Run the CLI example (interpreter MVP):

```
scripts/cargo_env.sh cargo run -p fusec -- --run examples/cli_hello.fuse
```

Run the project demo (AST backend, enum + refined types):

```
APP_GREETING=Hey APP_WHO=Codex scripts/cargo_env.sh cargo run -p fusec -- --run --backend ast examples/project_demo.fuse
```

Run the HTTP users service (AST backend):

```
APP_PORT=4000 scripts/cargo_env.sh cargo run -p fusec -- --run --backend ast examples/http_users.fuse
```

Run the interpolation demo (AST or VM):

```
scripts/cargo_env.sh cargo run -p fusec -- --run --backend ast examples/interp_demo.fuse
scripts/cargo_env.sh cargo run -p fusec -- --run --backend vm examples/interp_demo.fuse
```

Trigger a validation error (prints error JSON on stderr):

```
DEMO_FAIL=1 scripts/cargo_env.sh cargo run -p fusec -- --run --backend ast examples/project_demo.fuse
```

Check all examples:

```
scripts/check_examples.sh
```

Files:

- `examples/cli_hello.fuse`: CLI hello with config defaults.
- `examples/cli_args.fuse`: CLI args binding (flags + values).
- `examples/http_users.fuse`: HTTP service with routes and `?!` error handling.
- `examples/types_patterns.fuse`: enums, structs, and pattern matching (Option/Result).
- `examples/project_demo.fuse`: config env overrides, refined types, enums, and match.
- `examples/interp_demo.fuse`: string interpolation (AST + VM).
- `examples/spawn_await_box.fuse`: spawn/await/box parity demo.
- `examples/box_shared.fuse`: shared `box` state across tasks.
- `examples/assign_field.fuse`: struct field assignment.
- `examples/assign_index.fuse`: list/map index assignment.
- `examples/range_demo.fuse`: range expressions (inclusive lists).
- `examples/enum_match.fuse`: enum declarations and match expressions.
- `examples/float_compare.fuse`: float comparison semantics.
- `examples/task_api.fuse`: task API (`task.id`, `task.done`, `task.cancel`).
- `examples/native_bang_error.fuse`: native backend `?!` error handling.
- `examples/native_bench.fuse`: native backend performance smoke test.
- `examples/native_builtins.fuse`: native backend builtin coverage.
- `examples/native_db.fuse`: native backend DB execution.
- `examples/native_heap_literals.fuse`: native backend heap-allocated literals.
- `examples/native_json.fuse`: native backend JSON encode/decode.
- `examples/native_validation.fuse`: native backend validation behavior.

# Examples

These examples are meant to be parsed and sema-checked by `fusec`. Runtime/codegen is not implemented yet, so use `--check` for now.

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
- `examples/http_users.fuse`: HTTP service with routes and `?!` error handling.
- `examples/types_patterns.fuse`: enums, structs, and pattern matching (Option/Result).
- `examples/project_demo.fuse`: config env overrides, refined types, enums, and match.

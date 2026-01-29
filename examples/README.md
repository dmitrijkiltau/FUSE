# Examples

These examples are meant to be parsed and sema-checked by `fusec`. Runtime/codegen is not implemented yet, so use `--check` for now.

Check an example:

```
scripts/cargo_env.sh cargo run -p fusec -- --check examples/cli_hello.fuse
```

Check all examples:

```
scripts/check_examples.sh
```

Files:

- `examples/cli_hello.fuse`: CLI hello with config defaults.
- `examples/http_users.fuse`: HTTP service with routes and `?!` error handling.
- `examples/types_patterns.fuse`: enums, structs, and pattern matching (Option/Result).

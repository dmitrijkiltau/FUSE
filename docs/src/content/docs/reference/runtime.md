---
title: Runtime reference
description: Config, services, and errors.
---

## Config loading

Config values are resolved in this order:

1. Environment overrides (e.g., `APP_PORT`)
2. Config file (TOML via `FUSE_CONFIG`)
3. Default expressions

## Runtime env vars

- `FUSE_CONFIG`: path to a TOML config file (default: `config.toml`)
- `FUSE_HOST`: bind host for `serve` (default: `127.0.0.1`)
- `FUSE_SERVICE`: select a service when multiple exist
- `FUSE_MAX_REQUESTS`: stop server after N requests (useful for tests)

## Errors

Runtime errors are emitted as JSON when possible. Validation errors include
field paths and error codes.

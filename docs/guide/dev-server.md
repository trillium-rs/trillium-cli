---
title: dev-server
---

# `trillium dev-server`

A watch / rebuild / restart loop for trillium applications, with browser
live-reload. It watches your source, rebuilds with `cargo` on change, restarts
your binary, and serves a reload-injecting proxy in front of it so the browser
refreshes automatically when a new build comes up.

:::note Feature-gated, Unix only

`dev-server` is **not** in the default build and is available only on Unix.
Install it with `cargo install trillium-cli --features dev-server`, or run from
a checkout with `cargo run --features dev-server -- dev-server`.

:::

## How it works

`dev-server` runs three things in concert:

1. A **file watcher** over your source (default: `./src`). On a change, it runs
   `cargo build` and, on success, restarts your application binary.
2. Your **application**, which it expects to listen on `http://localhost:8080`.
3. A **live-reload proxy** on `http://localhost:8082` that forwards to your app
   and injects a small script into HTML responses. The script opens a WebSocket
   back to the dev-server and reloads the page when a rebuild completes.

Point your browser at **`http://localhost:8082`** (the proxy), not `:8080`, to
get live reload. These two ports are currently fixed.

```sh
# from your trillium app's project root
trillium dev-server
```

## Selecting what to build and run

By default `dev-server` uses `cargo metadata` to find your package's default
binary in the target directory. Override the target with `--bin` or `--example`,
and the build profile with `--release`:

```sh
trillium dev-server --example hello-world
trillium dev-server --bin my-server --release
```

| Flag              | Env     | Default | Notes                                               |
|-------------------|---------|---------|-----------------------------------------------------|
| `-w`, `--watch`   | `WATCH` | `src`   | path(s) to watch for changes, repeatable            |
| `-b`, `--bin`     | `BIN`   |         | path to the binary to run (skips `cargo metadata`)  |
| `-c`, `--cwd`     |         | cwd     | working directory to build and run in               |
| `-r`, `--release` |         |         | build and run the release profile                   |
| `-e`, `--example` |         |         | build and run a named example (also watches `examples/`) |
| `-s`, `--signal`  |         | `SIGTERM` | signal sent to the child to trigger a restart     |

## Full flag reference

```
trillium dev-server [OPTIONS]

Options:
  -w, --watch <WATCH>      [env: WATCH=]   [default: src]
  -b, --bin <BIN>          [env: BIN=]
  -c, --cwd <CWD>
  -r, --release
  -e, --example <EXAMPLE>
  -s, --signal <SIGNAL>    [default: SIGTERM]
  -h, --help
```

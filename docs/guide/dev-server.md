---
title: dev-server
---

# `trillium dev-server`

A watch / rebuild / restart loop for trillium applications, with browser
live-reload and an in-browser compiler-error overlay. It watches your source,
rebuilds with `cargo` on change, restarts your binary, and serves a
reload-injecting proxy in front of it so the browser refreshes automatically
when a new build comes up.

:::note Feature-gated, Unix only

`dev-server` is **not** in the default build and is available only on Unix.
Install it with `cargo install trillium-cli --features dev-server`, or run from
a checkout with `cargo run --features dev-server -- dev-server`.

:::

```sh
# from your trillium app's project root
trillium dev-server
```

Then open **`http://localhost:8080`** — the same address you'd normally point at
your app. The dev server listens there and runs your application invisibly
behind it.

## How it works

`dev-server` runs three things in concert:

1. A **file watcher** over your crate's source. On a change it runs
   `cargo build` and, on success, restarts your application binary.
2. Your **application**, which the dev server launches for you on a private
   port (it sets `PORT`/`HOST` in the child's environment) and waits for it to
   start listening before declaring it ready.
3. A **live-reload proxy** on the host/port *you* point your browser at. It
   forwards to your app and injects a small script into HTML responses. The
   script opens a WebSocket back to the dev server, reloads the page when a
   rebuild completes, and renders compile errors as an overlay.

## Addresses: it adopts your `HOST`/`PORT`

The dev server listens on the host and port you'd use to reach the app —
`localhost:8080` by default, overridable with `-o`/`--host` and `-p`/`--port`,
or the `HOST`/`PORT` environment variables. Your application is moved onto an
auto-allocated free port behind the proxy; the dev server passes that port to
it as `PORT` (and `localhost` as `HOST`), which any trillium app reads out of
the box.

```sh
PORT=3000 trillium dev-server     # visit http://localhost:3000
```

:::tip Why take over the app's port?

So the address is the same one you'd use in production: your `PORT` still
controls where you reach the app, and the rebuild plumbing slots in invisibly
behind it — no remembering a second "dev-only" port.

:::

If your app **hardcodes** its listen port rather than reading `PORT`, tell the
dev server with `--app-port` so it proxies to the right place (and `--app-host`
if it isn't `localhost`):

```sh
trillium dev-server --app-port 4000
```

The dev server also sets `TRILLIUM_CLI_DEV_SERVER=1` in your app's environment,
so your code can detect that it's running under the dev server if it ever needs
to.

## Selecting what to build

The dev server learns which binary to run from the build itself — whatever
`cargo build` produces is what it launches. Hand cargo's own selection flags to
`--build-args`, as a single shell-quoted string, and it all just works:

```sh
trillium dev-server --build-args "-p my-crate"
trillium dev-server --build-args "--bin worker --features dev"
```

`--example` and `--release` are first-class (the example also adds `examples/`
to the watch set; release disables the dev build speedups described below):

```sh
trillium dev-server --example hello-world
trillium dev-server --release
```

## Passing arguments to your app

Some binaries need a subcommand or a runtime flag before they start serving —
for example an app whose first argument is `serve`. Hand those to `--run-args`;
they're passed to your binary on **every** start, including after each rebuild:

```sh
trillium dev-server --run-args serve
trillium dev-server --build-args "-p my-app" --run-args "serve --verbose"
```

Both `--build-args` and `--run-args` take one shell-quoted string, split the way
a shell would — so quotes and spaces survive, e.g. a config path with a space in
it. Each flag can be repeated, appending to the list. Build args go to `cargo
build`; run args go to the binary it produces.

## What gets watched

By default the dev server watches the `src` directory of the crate it builds
**plus every workspace-local crate that one depends on** — so in a workspace,
editing a path-dependency library rebuilds and reloads the app that uses it, not
just the top-level binary. Registry dependencies are never watched.

Add more directories (templates, assets, a crate outside the dependency graph)
with `-w`/`--watch`; they're watched *in addition* to the default:

```sh
trillium dev-server --build-args "-p web" --watch ./templates --watch ./assets
```

Filesystem events are debounced, so saving several files at once triggers a
single rebuild.

## The browser overlay

The injected script gives you, with no setup:

- **Live reload** — the page refreshes when a new build comes up and the app is
  listening again (the dev server waits for the port, so you don't reload into a
  not-yet-started server).
- **A status pill** in the corner — `rebuilding…` while `cargo` runs,
  `reloading…` once it succeeds.
- **A compile-error overlay** — when a build fails, the errors are rendered over
  the page (the previous build keeps running underneath, so the app stays up).
  Each error shows rustc's own output with a clickable `file:line:column`.

The overlay reflects the *current* state on connect, too: open a tab while the
build is broken and you'll see the errors immediately, rather than waiting for
the next rebuild.

### Click to open in your editor

Click an error's `file:line:column` and the dev server opens it in your editor,
jumping to the line. It uses `--editor` if given, otherwise `$EDITOR` (then
`$VISUAL`), and formats the arguments for the editor it recognizes —
`emacs`/`emacsclient`, `vim`/`nvim`, `code`, `subl`, `zed`, and the JetBrains
launchers — falling back to just opening the file otherwise. The value may
include arguments:

```sh
trillium dev-server --editor "code --wait"
EDITOR=emacsclient trillium dev-server
```

:::note No editor URL schemes — and no arbitrary opens

There's no reliable cross-editor "open at line" URL a browser can use, so the
open happens server-side over the same WebSocket. To keep that safe, the browser
never sends a file path: each error carries an opaque id, and the dev server
opens only the location the compiler reported for that id. A page can't ask the
dev server to open an arbitrary file (which, for editors that evaluate
file-local variables or modelines, would be a code-execution vector).

:::

## Faster builds

Because the dev server owns the `cargo` invocation, it applies a couple of
safe dev-build speedups by default: it trims debug info to line tables (the
biggest link-time cost) and, if a fast linker (`mold`/`lld`) is installed and
you haven't set your own `RUSTFLAGS`, wires it in. These are injected through
the environment, so your `Cargo.toml` is untouched.

Toggling them changes the build fingerprint, so the first run after enabling or
disabling them is a full rebuild. Turn them off with `--no-fast`; `--release`
disables them implicitly.

## Logging

`-v` increases the log level, `-q` decreases it:

```sh
trillium dev-server -v        # info
trillium dev-server -vv       # debug
trillium dev-server -q        # warn only
```

The dev server also logs each watched directory, build success/failure, and
when the app comes up on its private port.

## Full flag reference

```
trillium dev-server [OPTIONS]

Options:
  -o, --host <HOST>              [env: HOST=]       [default: localhost]
  -p, --port <PORT>              [env: PORT=]       [default: 8080]
  -w, --watch <WATCH>            extra dirs to watch (repeatable, added to default)
  -c, --cwd <CWD>
  -r, --release
  -e, --example <EXAMPLE>
      --app-port <APP_PORT>      [env: APP_PORT=]   (use when the app hardcodes its port)
      --app-host <APP_HOST>      [default: localhost]
      --no-fast                  disable dev build speedups
      --editor <EDITOR>          [env: EDITOR=]     (also falls back to $VISUAL)
  -s, --signal <SIGNAL>          [default: SIGTERM]
  -v, --verbose...
  -q, --quiet...
      --build-args <BUILD_ARGS>  cargo build selection flags (shell-quoted, repeatable)
      --run-args <RUN_ARGS>      args passed to your app on every start (shell-quoted, repeatable)
  -h, --help
```

Always check `trillium dev-server --help` against the version you've installed —
this page documents the current stable release.

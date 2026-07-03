# trillium

[![crates.io version](https://img.shields.io/crates/v/trillium-cli.svg)](https://crates.io/crates/trillium-cli)
[![license](https://img.shields.io/crates/l/trillium-cli.svg)](#license)

Full documentation: **<https://cli.trillium.rs>**.

A single `trillium` binary that bundles the most useful pieces of the
[trillium.rs](https://trillium.rs) web stack into a batteries-included HTTP
toolkit:

- **`serve`** — a static file server (and drop-in reverse proxy)
- **`proxy`** — a reverse/forward proxy with upstream load-balancing and caching
- **`gateway`** — a config-driven server combining static files + proxy across one or more listeners
- **`client`** — a curl-like HTTP client that pretty-prints JSON and follows redirects
- **`bench`** — a load generator with HDR-histogram latency statistics
- **`dev-server`** — watch/rebuild/restart loop with browser live-reload for trillium apps (opt-in feature, Unix only)

TLS is built in (rustls by default), and with the default `h3` feature the
servers also speak HTTP/3 over QUIC. Over TLS the `client` negotiates HTTP/2 via
ALPN; `--http-version` selects the protocol (HTTP/1.0 through HTTP/3) for
`client` and `bench`.

## Install

The binary is named `trillium`. Run `trillium --help`, or
`trillium <command> --help`, for the full option list — the examples below
cover the common cases.

### Prebuilt binaries (recommended)

Each release ships prebuilt binaries for x86_64/aarch64 macOS, x86_64 Linux,
and x86_64 Windows — no Rust toolchain, no compile. They're built with **every
subcommand enabled** (`gateway`, `grpc`, and, on macOS/Linux, `dev-server`), so
you get the full toolkit out of the box.

If you have [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), it
reads the release metadata and fetches the right binary for your platform:

```sh
cargo binstall trillium-cli
```

Otherwise, the installer scripts detect your platform, download the matching
archive, and place the binary in `~/.cargo/bin`:

**macOS and Linux:**

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/trillium-rs/trillium-cli/releases/latest/download/trillium-cli-installer.sh | sh
```

**Windows (PowerShell):**

```powershell
powershell -c "irm https://github.com/trillium-rs/trillium-cli/releases/latest/download/trillium-cli-installer.ps1 | iex"
```

Both always install the most recent release. You can also pin a specific version
or download the archives directly from the
[releases page](https://github.com/trillium-rs/trillium-cli/releases).

### From crates.io

To compile from source with cargo:

```sh
cargo install trillium-cli
```

A default `cargo install` builds only `serve`, `proxy`, `client`, and `bench`
(the prebuilt binaries above bundle the rest). Select
[features](#building-from-source--feature-flags) to add the others:

```sh
cargo install trillium-cli --features gateway,grpc,dev-server
```

Most listening options also read from environment variables (`HOST`, `PORT`,
`CERT`, `KEY`, `FORWARD`, `UPSTREAM`), so they compose well with `.env` files
and process managers.

## `serve` — static files

Serve the current directory on <http://localhost:8080>:

```sh
trillium serve
```

Pick a directory and port, and serve over your LAN:

```sh
trillium serve ./public --host 0.0.0.0 --port 3000
```

Responses are compressed (gzip/brotli/zstd) automatically based on the client's
`Accept-Encoding`; pass `--no-compress` to turn that off.

**Directory listings.** By default a request for a directory with no index file
returns `404 Not Found`. Pass `-l` / `--directory-listing` (or set
`DIRECTORY_LISTING=1`) to instead render an HTML index of the directory's
contents, with clickable column headers that sort by name, size, or modification
time:

```sh
trillium serve ./files --directory-listing
```

It's off by default because it exposes file names and structure. Configuring an
`--index` file takes precedence — listings only appear for directories without
one.

**Single-page apps & reverse proxying.** `--forward` turns any request that
would 404 into a reverse proxy to another origin — perfect for serving a built
frontend while passing `/api` calls through to a backend:

```sh
trillium serve ./dist --forward http://localhost:4000
```

**Rate limiting.** Cap requests per client network. Over-quota requests get
`429 Too Many Requests` with a `Retry-After` header, and every metered response
advertises the standard `RateLimit` / `RateLimit-Policy` headers:

```sh
trillium serve --rate-limit 100/min          # sustained 100 req/min per network
trillium serve --rate-limit 10/s --rate-limit-burst 50   # allow short spikes
```

Rates are written `COUNT/WINDOW`, where the window is `s`, `min`, or `h`.

## `proxy` — reverse & forward proxy

Proxy all traffic to a single upstream:

```sh
trillium proxy http://localhost:4000
```

Load-balance across several upstreams (default strategy is round-robin):

```sh
trillium proxy http://app-1:4000 http://app-2:4000 --strategy connection-counting
```

Strategies: `round-robin`, `connection-counting`, `random`, and `forward` (a
classic forward proxy, including `CONNECT` tunneling — pass no upstreams).

The proxy ships with an in-memory response **cache** (honoring caching headers),
**compression**, WebSocket upgrade passthrough, and the same `--rate-limit`
controls as `serve`:

```sh
# 1 GiB cache, evict entries idle for 5 minutes, throttle abusive clients
trillium proxy http://localhost:4000 \
  --cache-capacity 1GiB --cache-time-to-idle 5m \
  --rate-limit 1000/min
```

Use `--no-cache` to disable caching entirely. When an upstream is `https://`,
select a client TLS backend with `--client-tls` (`-k`/`--insecure` skips
verification for self-signed dev certs).

## `gateway` — config-driven server

Where `serve` and `proxy` each do one thing from flags, `gateway` reads a
[KDL](https://kdl.dev) config file and assembles the same building blocks —
static files, reverse proxy, redirects, header & HTML rewriting, compression,
rate limiting, TLS/h3 — into one or more listeners. It's a trillium-backed
caddy/nginx-lite.

```sh
trillium gateway --config gateway.kdl
trillium gateway --config gateway.kdl --check   # parse + print the resolved config, don't serve
```

A `binding` is one listener (`host:port` + optional TLS + per-binding HTTP
tuning). Within it, ordered `route` patterns dispatch by path to a stack of
directives:

```kdl
compression true
rate-limit "100/min" burst=200

// Opt-in response cache for proxied upstreams (off unless declared, unlike
// `trillium proxy`). A bare `cache` node enables it with defaults.
cache {
    capacity "256MiB"
    time-to-idle "5m"
}

binding ":443" {
    tls cert="./cert.pem" key="./key.pem"
    http {
        received-body-max-len "10MiB"
    }

    route "/api/*" {
        // /api is stripped (the route pattern controls stripping, like `files`);
        // give the upstream a base path to forward *with* the prefix instead.
        proxy strategy="round-robin" {
            upstream "http://127.0.0.1:9000"
            upstream "http://127.0.0.1:9001"
        }
    }

    route "/old/*" {
        redirect "https://example.com/new" status=308
    }

    route "/*" {
        headers {
            add "X-Served-By" "trillium"
            remove "Server"
        }
        files root="./public" index="index.html" directory-listing=true
    }
}
```

Declare multiple `binding` blocks to run several listeners in one process; a
single `Ctrl-C` drains all of them gracefully. A bare `:443` host binds all
interfaces (the nginx `listen :80` convention). Routes match by path
specificity, for all HTTP methods.

**HTML rewriting.** A `rewrite-html` directive streams the response body through
[lol-html](https://docs.rs/lol-html), applying ordered mutations to the elements
matched by CSS selectors — inject tags, rewrite attributes, or strip nodes from
a static page or a proxied upstream. Only `text/html` responses are touched;
JSON and binary stream through untouched, so it's safe to drop in front of a
mixed `proxy`. CSS selectors are validated when the config loads (with a source
span pointing at any unsupported selector), not on the first request.

```kdl
route "/*" {
    proxy {
        upstream "http://127.0.0.1:9000"
    }

    rewrite-html {
        select "head" {
            append "<script src=\"/analytics.js\" async></script>"
        }
        select "a[target=_blank]" {
            set-attribute "rel" "noopener noreferrer"
        }
        select "img" {
            set-attribute "loading" "lazy"
        }
        select ".legacy-banner" {
            remove
        }
        select "title" {
            set-text "Proxied by trillium"
        }
    }
}
```

Each `select "css-selector"` block holds an ordered list of element mutations.
Markup-valued ops (`before`, `after`, `prepend`, `append`, `set-inner`,
`replace`) insert their argument as HTML; `set-text` inserts HTML-escaped text.
The rest: `set-attribute "name" "value"`, `remove-attribute "name"`,
`set-tag "div"`, `remove` (delete the element and its content), and `unwrap`
(drop the element's tags but keep its content).

**Virtual hosting.** Put `host` blocks inside a binding to dispatch by `Host`
header on a shared socket. Patterns are exact (`example.com`), wildcard
(`*.example.com`, any subdomain), or `*` (any). A request matching no `host`
block falls back to the binding's direct `routes` — which also catches requests
with no `Host` header (HTTP/1.0):

```kdl
binding ":443" {
    tls cert="./cert.pem" key="./key.pem"

    host "app.example.com" {
        route "/*" {
            proxy {
                upstream "http://127.0.0.1:9000"
            }
        }
    }
    host "*.static.example.com" {
        route "/*" {
            files root="./public"
        }
    }

    // default vhost: unmatched hosts (and Host-less requests)
    route "/*" {
        redirect "https://example.com" status=308
    }
}
```

## `client` — make requests

A curl-like client that pretty-prints JSON, streams bodies, and follows
redirects by default:

```sh
trillium client get https://example.com
trillium client get https://httpbin.org/json
```

Send headers and a body (from the command line, a file, or stdin):

```sh
trillium client post https://httpbin.org/anything \
  -H Authorization="Bearer $TOKEN" Content-Type=application/json \
  -b '{"hello": "world"}'

trillium client post https://httpbin.org/anything -f ./body.json
cat ./body.json | trillium client post https://httpbin.org/anything
```

Stream a Server-Sent Events endpoint with `--sse`. At a terminal each event is
rendered with colored field labels (event type, id, retry) and JSON payloads
are pretty-printed; piped, events become newline-delimited JSON for `jq`:

```sh
trillium client get https://example.com/events --sse
trillium client get https://example.com/events --sse | jq .data
```

Other handy flags: `--output-file` to save the body, `--dry-run` to print the
request without sending it, `-c`/`--compression` to compress the request body
(`zstd`/`br`/`gzip`), `--retry` to retry failed requests with backoff,
`--timeout`/`--no-timeout`, and `--no-follow-redirects` / `--max-redirects` to
control redirect behavior.

## `bench` — generate load

Closed-loop: 50 concurrent connections for 10 seconds (defaults):

```sh
trillium bench https://localhost:8080
```

Open-loop at a target arrival rate (switches to scheduled load, useful for
measuring latency under a fixed offered rate):

```sh
trillium bench https://localhost:8080 --rate 5000 --pacing poisson --duration 30s
```

Results are reported as an HDR-histogram latency summary. Add `--json` for a
machine-readable report on stdout, or `--csv <path>` for per-request timing
data. `--connections`, `--requests`, `--warmup`, and `--timeout` round out the
common knobs.

## `dev-server` — live-reload for trillium apps

A watch / rebuild / restart loop with browser live-reload. It's Unix-only. The
[prebuilt macOS/Linux binaries](#prebuilt-binaries-recommended) already include
it; with `cargo install` it's feature-gated, so enable it explicitly:

```sh
cargo install trillium-cli --features dev-server
```

Run it from your app's project root and open the address it listens on:

```sh
trillium dev-server          # then visit http://localhost:8080
```

It watches your source, rebuilds with `cargo` on change, restarts your binary,
and serves a reload-injecting proxy in front of it. The dev server **adopts your
`HOST`/`PORT`** (so you visit the same address you'd use in production) and runs
your app on a private port behind the proxy, passing it through as `PORT`. Use
`--app-port` if your app hardcodes its own port instead of reading `PORT`.

In a workspace it watches the crate it builds **plus the workspace-local crates
it depends on**, so editing a path-dependency library reloads the app using it.
Select what to build with cargo's own flags via `--build-args`, and pass any
runtime arguments your binary needs (a subcommand, a flag) with `--run-args`:

```sh
trillium dev-server --build-args "-p my-app --features dev"
trillium dev-server --example hello-world
trillium dev-server --build-args "-p my-app" --run-args serve
```

When a build fails, the errors render as an overlay in the browser (the previous
build keeps running underneath). Click a `file:line:column` and it opens in your
editor — `$EDITOR` by default, or `--editor "code --wait"` — jumping to the
line. It also applies safe dev-build speedups (trimmed debug info + a fast linker
if one's installed); disable them with `--no-fast`.

See the [`dev-server` guide](https://cli.trillium.rs/dev-server) for the full
details.

## HTTPS

Provide a certificate and key to serve over TLS (and, with the default `h3`
feature, HTTP/3 over QUIC on the same port):

```sh
trillium serve --cert ./cert.pem --key ./key.pem
# or via the environment:
CERT=./cert.pem KEY=./key.pem trillium serve
```

For local development, [`mkcert`](https://github.com/FiloSottile/mkcert) or
`rcgen` will generate a trusted cert/key pair. Test an HTTPS+h3 server with
`curl -k https://localhost:8080`.

## Building from source & feature flags

```sh
git clone https://github.com/trillium-rs/trillium-cli
cd trillium-cli
cargo build --release        # release builds use fat LTO
```

Each subcommand is gated behind a Cargo feature, so you can build a smaller
binary with only what you need:

| Feature      | Subcommand   | Default | Notes |
|--------------|--------------|:-------:|-------|
| `serve`      | `serve`      | ✅ | static file server + reverse proxy |
| `proxy`      | `proxy`      | ✅ | reverse/forward proxy with caching |
| `gateway`    | `gateway`    |    | config-driven multi-listener server (KDL) |
| `client`     | `client`     | ✅ | HTTP client |
| `bench`      | `bench`      | ✅ | load generator |
| `dev-server` | `dev-server` |    | watch/rebuild/restart loop (Unix only) |
| `grpc`       | `grpc`       |    | generate Rust modules from `.proto` files |

TLS backends are selectable too: `rustls` (default), `native-tls`, and
`openssl`. The `h3` feature (default) adds HTTP/3 over QUIC and implies
`rustls`.

```sh
# just the client, built against the system's native TLS
cargo install trillium-cli --no-default-features --features client,native-tls
```

## License

Licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE) at your option.

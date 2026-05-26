# trillium

[![crates.io version](https://img.shields.io/crates/v/trillium-cli.svg)](https://crates.io/crates/trillium-cli)
[![license](https://img.shields.io/crates/l/trillium-cli.svg)](#license)

A single `trillium` binary that bundles the most useful pieces of the
[trillium.rs](https://trillium.rs) web stack into a batteries-included HTTP
toolkit:

- **`serve`** — a static file server (and drop-in reverse proxy)
- **`proxy`** — a reverse/forward proxy with upstream load-balancing and caching
- **`client`** — a curl-like HTTP client that pretty-prints JSON and follows redirects
- **`bench`** — a load generator with HDR-histogram latency statistics

TLS is built in (rustls by default), and with the default `h3` feature the
servers also speak HTTP/3 over QUIC. Over TLS the `client` negotiates HTTP/2 via
ALPN; `--http-version` selects the protocol (HTTP/1.0 through HTTP/3) for
`client` and `bench`.

## Install

```sh
cargo install trillium-cli
```

This installs a binary named `trillium`. Run `trillium --help`, or
`trillium <command> --help`, for the full option list — the examples below
cover the common cases.

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

Other handy flags: `--output-file` to save the body, `--dry-run` to print the
request without sending it, `--timeout`/`--no-timeout`, and
`--no-follow-redirects` / `--max-redirects` to control redirect behavior.

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

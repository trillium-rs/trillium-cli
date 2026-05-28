---
title: serve
---

# `trillium serve`

A static file server with optional reverse-proxy fallback, HTTPS, HTTP/3,
compression, rate limiting, and directory listings.

```sh
trillium serve [ROOT]
```

The simplest invocation serves the current directory on
`http://localhost:8080`:

```sh
trillium serve
```

Pick a directory, host, and port:

```sh
trillium serve ./public --host 0.0.0.0 --port 3000
```

## Listening

| Flag             | Env       | Default     |
|------------------|-----------|-------------|
| `-o`, `--host`   | `HOST`    | `localhost` |
| `-p`, `--port`   | `PORT`    | `8080`      |

Bind to all interfaces with `--host 0.0.0.0`, or to a specific interface with
its address. The env-var fallbacks let you drive `trillium serve` from a
process manager or `.env` file with no flags at all.

## HTTPS and HTTP/3

Provide a TLS certificate and key to serve over HTTPS, and — with the default
`h3` feature — over HTTP/3 on the same port:

```sh
trillium serve --cert ./cert.pem --key ./key.pem
# or via the environment:
CERT=./cert.pem KEY=./key.pem trillium serve
```

| Flag      | Env    | Notes                                                         |
|-----------|--------|---------------------------------------------------------------|
| `--cert`  | `CERT` | path to the certificate (PEM); requires `--key`               |
| `--key`   | `KEY`  | path to the private key (PEM); requires `--cert`              |
| `--tls`   | `TLS`  | acceptor backend: `rustls` (default), `native`, `openssl`     |

For local development, [`mkcert`](https://github.com/FiloSottile/mkcert) or
`rcgen` will generate a trusted cert/key pair. Test an HTTPS + h3 server with
`curl -k https://localhost:8080`.

Which backends are available depends on Cargo features. The default build
includes `rustls` + `h3`. See [Installing](./installing#tls-backends).

## Index files and directory listings

By default a request that resolves to a directory looks for an index file
configured with `--index` (env `INDEX`):

```sh
trillium serve ./public --index index.html
```

If there is no index file, the request is treated as a 404. Pass
`-l` / `--directory-listing` (env `DIRECTORY_LISTING=1`) to instead render a
clickable HTML index of the directory's contents, with column headers that
sort by name, size, or modification time:

```sh
trillium serve ./files --directory-listing
```

Listings are off by default because they expose file names and structure.
When an `--index` file is configured it takes precedence — listings only
appear for directories without one.

## Single-page apps and reverse-proxy fallback

`-f` / `--forward` (env `FORWARD`) puts a reverse proxy in front of the static
handler. **Every request goes upstream first**; local files only respond when
the upstream returns a 404. That makes it a drop-in for serving a built SPA
while letting an API backend handle its own routes:

```sh
trillium serve ./dist --forward http://localhost:4000
```

```sh
# or via the environment
FORWARD=http://localhost:4000 trillium serve ./dist
```

Upstream URLs accept `http://`, `https://`, or a bare `host:port` (which
defaults to `http://`). `http+unix://` is not yet supported. For HTTPS
upstreams with self-signed certs, see the trillium-cli `client` and `proxy`
pages for the relevant `--client-tls` controls — `serve --forward` always uses
the default TLS backend.

:::tip Why upstream-first?

It might seem more natural to serve files first and proxy on a local miss.
Going upstream first means the upstream owns its own URL space — any path it
chooses to handle (including dynamic routes that look like file paths) wins
over any matching file on disk. The static handler fills the gaps where the
upstream 404s. For an API-only backend that 404s everywhere except `/api/*`,
the two orderings behave the same way; the upstream-first ordering only
matters when the upstream and the static tree overlap.

:::

## Compression

Responses are compressed (gzip / brotli / zstd) automatically based on the
client's `Accept-Encoding` header. Compression applies to both static and
proxied responses. Disable it with:

```sh
trillium serve --no-compress
```

## Rate limiting

Cap requests per client network with `--rate-limit RATE`. Over-quota requests
get `429 Too Many Requests` with a `Retry-After` header, and every metered
response advertises the standard `RateLimit` and `RateLimit-Policy` headers:

```sh
trillium serve --rate-limit 100/min                      # sustained 100 req/min per network
trillium serve --rate-limit 10/s --rate-limit-burst 50   # allow short spikes
```

Rates are written `COUNT/WINDOW`, where `WINDOW` is `s`, `min`, or `h` (or
their longer forms — `seconds`, `minutes`, `hours`). `--rate-limit-burst`
permits short spikes above the sustained rate; it defaults to the rate
itself.

Rate limiting is off unless you pass `--rate-limit`.

## Logging

`-v` increases the log level, `-q` decreases it. Logs are formatted with the
module path dimmed and the message in the default color:

```sh
trillium serve -v        # info
trillium serve -vv       # debug
trillium serve -vvv      # trace
trillium serve -q        # warn only
```

Quinn (the HTTP/3 implementation) is silenced by default at every log level so
its internals don't drown out the request log.

## Full flag reference

```
trillium serve [OPTIONS] [ROOT]

Arguments:
  [ROOT]  Filesystem path to serve (default: cwd)

Options:
  -o, --host <HOST>                  [env: HOST=]                [default: localhost]
  -p, --port <PORT>                  [env: PORT=]                [default: 8080]
      --cert <CERT>                  [env: CERT=]
      --key  <KEY>                   [env: KEY=]
      --tls  <TLS>                   [env: TLS=]                 [default: rustls]
  -f, --forward <FORWARD>            [env: FORWARD=]
  -i, --index <INDEX>                [env: INDEX=]
      --no-compress
  -l, --directory-listing            [env: DIRECTORY_LISTING=]
      --rate-limit <RATE>
      --rate-limit-burst <BURST>     (requires --rate-limit)
  -v, --verbose...
  -q, --quiet...
  -h, --help
```

Always check `trillium serve --help` against the version you've installed —
this page documents the current stable release.

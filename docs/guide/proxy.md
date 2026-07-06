---
title: proxy
---

# `trillium proxy`

A reverse and forward proxy with upstream load-balancing, an in-memory
response cache, compression, WebSocket passthrough, rate limiting, HTTPS, and
HTTP/3.

```sh
trillium proxy [UPSTREAM]...
```

Proxy all traffic to a single upstream:

```sh
trillium proxy http://localhost:4000
```

Upstream URLs accept `http://`, `https://`, or a bare `host:port` (which
defaults to `http://`). The upstream list also reads from the `UPSTREAM`
environment variable.

## Load balancing

Pass several upstreams to spread traffic across them. The default strategy is
round-robin:

```sh
trillium proxy http://app-1:4000 http://app-2:4000
trillium proxy http://app-1:4000 http://app-2:4000 --strategy connection-counting
```

| Strategy              | Behavior                                                          |
|-----------------------|-------------------------------------------------------------------|
| `round-robin`         | (default) cycle through the upstreams in order                    |
| `connection-counting` | send each request to the upstream with the fewest open connections|
| `random`              | pick an upstream at random per request                            |
| `forward`             | classic forward proxy — pass **no** upstreams (see below)         |

`-s` / `--strategy` also reads the `STRATEGY` environment variable.

## Forward-proxy mode

With `--strategy forward` and no upstreams, `trillium proxy` acts as a classic
forward proxy: it forwards absolute-form requests and tunnels HTTPS via
`CONNECT`.

```sh
trillium proxy --strategy forward
curl -x http://localhost:8080 https://example.com    # use it as a proxy
```

Passing upstreams together with `--strategy forward` is an error, as is
omitting upstreams for any other strategy.

## Caching

The proxy ships with an in-memory response cache that honors standard caching
headers (`Cache-Control`, `ETag`, …). **It is on by default** — appropriate for
a proxy fronting cacheable content. (The [`gateway`](./gateway/overview)
equivalent is opt-in, because a gateway shouldn't silently cache dynamic
upstreams.)

```sh
# 1 GiB in-memory cache, evict entries idle for 5 minutes
trillium proxy http://localhost:4000 \
  --cache-memory-capacity 1GiB --cache-time-to-idle 5m
```

By default the cache lives in memory and is lost on restart. Point
`--cache-disk` at a directory to add a **durable on-disk tier** — cached
responses then survive restarts, so the proxy comes back warm instead of
re-fetching everything from the upstream:

```sh
# hot in-memory cache over a durable 10 GiB on-disk tier
trillium proxy http://localhost:4000 \
  --cache-disk /var/cache/trillium --cache-disk-capacity 10GiB
```

Which tiers exist follows the flags: `--cache-disk` alone tiers a hot in-memory
cache over durable disk; add `--cache-memory-capacity 0` to drop the in-memory
tier and cache **only** to disk (for memory-constrained hosts).

| Flag                      | Default  | Notes                                                          |
|---------------------------|----------|----------------------------------------------------------------|
| `--no-cache`              |          | disable caching entirely                                       |
| `--cache-memory-capacity` | `256MiB` | in-memory (hot) tier size; `0` drops it (disk-only)            |
| `--cache-disk`            |          | directory for a durable on-disk tier; persists across restarts |
| `--cache-disk-capacity`   | `1GiB`   | on-disk tier size (only used with `--cache-disk`)              |
| `--cache-max-body`        | `16MiB`  | largest cacheable body; bigger responses stream uncached       |
| `--cache-time-to-idle`    |          | evict entries not read within this duration (e.g. `5m`)        |
| `--cache-time-to-live`    |          | evict entries this long after they are stored (e.g. `1h`)      |

The cache flags conflict with `--no-cache`. `--cache-capacity` is accepted as a
deprecated alias for `--cache-memory-capacity`.

## Listening, HTTPS, and HTTP/3

The proxy listens like [`serve`](./serve): `-o` / `--host` (env `HOST`, default
`localhost`) and `-p` / `--port` (env `PORT`, default `8080`). Provide a
certificate and key to terminate TLS — and, with the default `h3` feature,
HTTP/3 over QUIC on the same port:

```sh
trillium proxy http://localhost:4000 --cert ./cert.pem --key ./key.pem
# or via the environment:
CERT=./cert.pem KEY=./key.pem trillium proxy http://localhost:4000
```

| Flag      | Env    | Notes                                                     |
|-----------|--------|-----------------------------------------------------------|
| `--cert`  | `CERT` | listener certificate (PEM); requires `--key`              |
| `--key`   | `KEY`  | listener private key (PEM); requires `--cert`             |
| `--tls`   | `TLS`  | acceptor backend: `rustls` (default), `native`, `openssl` |

## Connecting to HTTPS upstreams

When an upstream is `https://`, the proxy needs a **client** TLS backend
(separate from the listener's `--tls` acceptor):

```sh
trillium proxy https://api.internal --client-tls rustls
trillium proxy https://localhost:4000 -k     # self-signed dev cert
```

| Flag                     | Notes                                                            |
|--------------------------|------------------------------------------------------------------|
| `-c`, `--client-tls`     | client backend for `https://` upstreams: `rustls`/`native`/`openssl` |
| `-k`, `--insecure`       | skip upstream certificate verification (rustls only) — **dangerous** |

`--insecure` disables authentication of the upstream entirely; use it only
against hosts you control.

## Compression, WebSockets, and 404s

- Responses are compressed (gzip / brotli / zstd) based on the client's
  `Accept-Encoding`. Disable with `--no-compress`.
- WebSocket upgrades are passed through to the upstream transparently.
- Upstream `404 Not Found` responses are forwarded to the client as-is. (This
  is the opposite of [`serve --forward`](./serve#single-page-apps-and-reverse-proxy-fallback),
  where a 404 falls back to local files.)
- Proxied responses carry a `Via: trillium-proxy` header.

## Rate limiting

The same controls as [`serve`](./serve#rate-limiting): `--rate-limit RATE` caps
requests per client network, `--rate-limit-burst` permits short spikes.
Off unless `--rate-limit` is given.

```sh
trillium proxy http://localhost:4000 --rate-limit 1000/min
```

## Full flag reference

```
trillium proxy [OPTIONS] [UPSTREAM]...

Arguments:
  [UPSTREAM]...  Upstream URLs            [env: UPSTREAM=]

Options:
  -s, --strategy <STRATEGY>          [env: STRATEGY=]   [default: round-robin]
  -o, --host <HOST>                  [env: HOST=]       [default: localhost]
  -p, --port <PORT>                  [env: PORT=]       [default: 8080]
      --cert <CERT>                  [env: CERT=]
      --key  <KEY>                   [env: KEY=]
      --tls  <TLS>                   [env: TLS=]        [default: rustls]
  -c, --client-tls <CLIENT_TLS>                         [default: rustls]
  -k, --insecure
      --no-compress
      --rate-limit <RATE>
      --rate-limit-burst <BURST>     (requires --rate-limit)
  -v, --verbose...
  -q, --quiet...
  -h, --help

Cache:
      --no-cache
      --cache-memory-capacity <CACHE_MEMORY_CAPACITY>   [default: 256MiB]
      --cache-disk <DIR>
      --cache-disk-capacity <CACHE_DISK_CAPACITY>       [default: 1GiB]
      --cache-max-body <CACHE_MAX_BODY>                 [default: 16MiB]
      --cache-time-to-idle <CACHE_TIME_TO_IDLE>
      --cache-time-to-live <CACHE_TIME_TO_LIVE>
```

For a config-driven proxy that can mix in static files, redirects, header
rewriting, and virtual hosts across several listeners, see
[`gateway`](./gateway/overview).

---
title: Overview
slug: /gateway/overview
---

# `trillium gateway`

Where [`serve`](../serve) and [`proxy`](../proxy) each do one thing configured
from flags, `gateway` reads a [KDL](https://kdl.dev) config file and assembles
the same building blocks — static files, reverse proxy, redirects, header and
HTML rewriting, compression, caching, rate limiting, TLS / h3 — into one or more
listeners. It's a trillium-backed caddy / nginx-lite.

:::note Feature-gated

`gateway` is **not** in the default build. Install it with
`cargo install trillium-cli --features gateway`, or run from a checkout with
`cargo run --features gateway -- gateway`. The `gateway` feature implies
`rustls`; with the default `h3` feature it also serves HTTP/3 over QUIC.

:::

## Running

```sh
trillium gateway --config gateway.kdl
trillium gateway --config gateway.kdl --check   # parse + print the resolved config, don't serve
```

| Flag             | Env                       | Default       | Notes                                   |
|------------------|---------------------------|---------------|-----------------------------------------|
| `-c`, `--config` | `TRILLIUM_GATEWAY_CONFIG` | `gateway.kdl` | path to the KDL config file             |
| `--check`        |                           |               | parse, validate, print the config; exit |

`--check` parses the file, validates it (including every
[`rewrite-html`](./rewrite-html) CSS selector), and prints the resolved
configuration without binding any sockets — use it in CI or before a reload.
Config errors are reported with [`miette`](https://docs.rs/miette) source spans
pointing at the offending line.

On startup, `gateway` prints a colored summary of every binding and the routes
it serves, so you can see at a glance what each listener does.

## Anatomy of a config

A config has optional cross-cutting defaults at the top, then one or more
`binding` blocks. Each binding is a listener; within it, ordered `route`
patterns dispatch by path to a stack of directives.

```kdl
compression true
rate-limit "100/min" burst=200

binding ":443" {
    tls cert="./cert.pem" key="./key.pem"
    http {
        received-body-max-len "10MiB"
    }

    route "/api/*" {
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

The pieces:

- **[Routing & directives](./routing)** — `route` patterns and the
  `files` / `proxy` / `redirect` / `headers` directives that make up a route's
  handler stack.
- **[HTML rewriting](./rewrite-html)** — the `rewrite-html` directive, a
  declarative streaming HTML transformer.
- **[Virtual hosts](./virtual-hosts)** — `host` blocks that dispatch by `Host`
  header on a shared socket, with per-host SNI certificates.

:::tip KDL child blocks need a line break

The KDL parser rejects a child block written entirely on one line:
`proxy { upstream "..." }` is a parse error. Put the child on its own line (or
end it with a `;`). Every example here uses the multiline form.

:::

## Bindings

A `binding` is one listener: a `host:port` address plus optional TLS and
per-binding HTTP tuning. Declare several to run multiple listeners in one
process.

```kdl
binding "0.0.0.0:8080" {
    route "/*" {
        files root="./public"
    }
}

binding ":443" {
    tls cert="./cert.pem" key="./key.pem"
    route "/*" {
        files root="./public"
    }
}
```

The listen address is `host:port`. A bare `:443` (empty host) binds all
interfaces — the nginx `listen :80` convention. With the `h3` feature, a TLS
binding also speaks HTTP/3 over QUIC on the same port.

### Per-binding HTTP tuning

An `http { … }` block overrides [`trillium_http::HttpConfig`](https://docs.rs/trillium-http)
defaults for that listener. Only the keys you set are changed; size-valued keys
accept human units (`"10MiB"`).

```kdl
binding ":8080" {
    http {
        received-body-max-len "10MiB"
        head-max-len "64KiB"
        max-connections 10000
    }
    route "/*" {
        files root="./public"
    }
}
```

## Cross-cutting defaults

Three nodes at the top of the document configure behavior inherited by every
binding.

### `compression`

```kdl
compression true    // default; set `compression false` to disable everywhere
```

Compression (gzip / brotli / zstd, by `Accept-Encoding`) is **on by default**.
`compression false` turns it off across all bindings.

### `rate-limit`

```kdl
rate-limit "100/min" burst=200
```

A per-client-network rate limit applied to every binding. The rate is written
`COUNT/WINDOW` (window `s`, `min`, or `h`); `burst` permits short spikes above
the sustained rate and defaults to the rate count. Over-quota requests get
`429 Too Many Requests` with a `Retry-After` header, and metered responses carry
the standard `RateLimit` / `RateLimit-Policy` headers. (Same engine as
[`serve`](../serve#rate-limiting) and [`proxy`](../proxy#rate-limiting).)

### `cache`

A response cache for `proxy` directives. **Opt-in** — absent means no caching.
(This is the opposite of [`trillium proxy`](../proxy#caching), where caching is
on by default; a gateway shouldn't silently cache dynamic upstreams.) A bare
`cache` node enables it with defaults; the children tune it.

```kdl
cache {
    capacity "256MiB"      // total in-memory size (default 256MiB)
    max-body "16MiB"       // largest cacheable body; bigger streams uncached (default 16MiB)
    time-to-idle "5m"      // evict entries not read within this duration
    time-to-live "1h"      // evict entries this long after they're stored
}
```

One cache (and one connection pool) is shared across every `proxy` directive in
the whole process. When caching is enabled, the gateway also adds
`ETag` / `Cache-Control` handling to its own responses.

## Graceful shutdown

All bindings share a single shutdown signal: one `Ctrl-C` (or `SIGINT`,
`SIGTERM`, `SIGQUIT` on Unix) drains every listener gracefully, letting
in-flight requests finish before the process exits.

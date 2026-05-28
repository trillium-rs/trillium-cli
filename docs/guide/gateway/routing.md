---
title: Routing & directives
slug: /gateway/routing
---

# Routing & directives

Within a [binding](./overview#bindings) (or a [virtual host](./virtual-hosts)),
ordered `route` blocks dispatch requests by path. Each route names a pattern and
holds a stack of **directives** — `files`, `proxy`, `redirect`, `headers`,
[`rewrite-html`](./rewrite-html) — compiled, in document order, into a single
handler for that path.

```kdl
binding ":8080" {
    route "/api/*" {
        proxy {
            upstream "http://127.0.0.1:9000"
        }
    }
    route "/*" {
        files root="./public" index="index.html"
    }
}
```

## Route patterns

Patterns are [routefinder](https://docs.rs/routefinder) patterns, the same
syntax trillium's router uses everywhere:

- `/*` — a wildcard matching any path (the catch-all).
- `/api/*` — everything under `/api`.
- `/users/:id` — a named segment.

Routes match for **all HTTP methods** (including `HEAD`, `OPTIONS`, `CONNECT`,
`TRACE`), so dispatch is purely by path. When several routes could match, the
most specific pattern wins — order in the file doesn't decide specificity.

### Prefix stripping

The matched prefix is **stripped** before the directive stack sees the request,
exactly like a static file root. A `/api/*` route proxying to an upstream
forwards `/users` (not `/api/users`) for a request to `/api/users`. To forward
*with* the prefix intact, give the upstream its own base path — see
[`proxy`](#proxy) below.

## `files`

Serve static files from a directory.

```kdl
route "/*" {
    files root="./public" index="index.html" directory-listing=true
}
```

| Property            | Notes                                                          |
|---------------------|----------------------------------------------------------------|
| `root`              | (required) directory to serve                                  |
| `index`             | index filename for directory requests (e.g. `index.html`)      |
| `directory-listing` | `true` renders an HTML listing for directories with no index   |

This is the same static handler as [`serve`](../serve); see that page for how
index files and directory listings interact.

## `proxy`

Reverse-proxy the route to one or more upstreams.

```kdl
route "/api/*" {
    proxy strategy="round-robin" {
        upstream "http://127.0.0.1:9000"
        upstream "http://127.0.0.1:9001"
    }
}
```

| Property / child | Notes                                                                  |
|------------------|------------------------------------------------------------------------|
| `strategy`       | `round-robin` (default), `connection-counting`, or `random`            |
| `upstream "url"` | one or more upstream targets                                           |

Upstream `404`s are forwarded to the client (a proxy route is terminal).
WebSocket upgrades pass through, and responses carry a `Via: trillium-gateway`
header. With a top-level [`cache`](./overview#cache) node, proxied responses are
cached.

### Path forwarding

The forwarded path is the prefix-stripped path concatenated onto each upstream
URL's own base path. This gives you explicit control over the prefix:

```kdl
// request /api/users  →  upstream gets /users  (route prefix stripped)
route "/api/*" {
    proxy {
        upstream "http://backend:9000"
    }
}

// request /api/users  →  upstream gets /api/users  (prefix preserved via base path)
route "/api/*" {
    proxy {
        upstream "http://backend:9000/api"
    }
}
```

## `redirect`

Respond with a `Location` redirect and halt.

```kdl
route "/old/*" {
    redirect "https://example.com/new" status=308
}
```

| Argument / property | Notes                                            |
|---------------------|--------------------------------------------------|
| _(first argument)_  | (required) target URL                            |
| `status`            | redirect status code; defaults to `302 Found`    |

## `headers`

Mutate response headers. Operations apply in order and run late (in
`before_send`), so they override headers set by the route's terminal handler —
including removing headers added by a proxied upstream (like `Server`).

```kdl
route "/*" {
    files root="./public"
    headers {
        add "X-Served-By" "trillium"
        set "Cache-Control" "public, max-age=3600"
        remove "Server"
    }
}
```

| Operation             | Notes                                                  |
|-----------------------|--------------------------------------------------------|
| `add "Name" "value"`  | append, keeping any existing values for that header    |
| `set "Name" "value"`  | replace any existing values                            |
| `remove "Name"`       | remove the header                                      |

## Directive ordering

Directives run in the order written. A body-producing directive
(`files` / `proxy`) is terminal for the response body; place response-shaping
directives like [`rewrite-html`](./rewrite-html) **after** it, and `headers`
anywhere (it runs late regardless).

```kdl
route "/*" {
    proxy {
        upstream "http://127.0.0.1:9000"
    }
    rewrite-html {
        select "title" {
            set-text "Proxied by trillium"
        }
    }
    headers {
        add "X-Served-By" "trillium"
    }
}
```

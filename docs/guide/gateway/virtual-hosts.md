---
title: Virtual hosts
slug: /gateway/virtual-hosts
---

# Virtual hosts

Put `host` blocks inside a [binding](./overview#bindings) to dispatch by the
`Host` header on a single shared socket. Each `host` block matches one or more
host patterns and has its own [routes](./routing); a request whose host matches
no block falls back to the binding's direct routes — the **default vhost**.

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

## Host patterns

A `host` block takes one or more patterns as arguments. Matching is
case-insensitive and ignores any port in the `Host` header.

| Pattern             | Matches                                          |
|---------------------|--------------------------------------------------|
| `example.com`       | exactly that host                                |
| `*.example.com`     | any single-or-multi-label subdomain              |
| `*`                 | any host                                         |

A block can list several:

```kdl
host "example.com" "www.example.com" {
    route "/*" {
        files root="./public"
    }
}
```

## The default vhost

The binding's direct `route` blocks (those not inside any `host`) serve as the
fallback for any request whose `Host` matches no block. This also catches
requests with **no** `Host` header — for example HTTP/1.0 clients. A binding
with no `host` blocks is just a plain set of routes (the behavior described in
[Routing](./routing)).

This works uniformly across HTTP/1.1, HTTP/2, and HTTP/3: the gateway resolves
the host from the `Host` header or, for h2/h3, the `:authority` pseudo-header.

## Per-host TLS (SNI)

A `host` block can carry its own `tls` certificate, selected by the TLS
ClientHello's SNI on the shared socket. The binding-level `tls` (if present) is
the fallback for unmatched SNI.

```kdl
binding ":443" {
    // fallback certificate for unmatched SNI
    tls cert="./default.pem" key="./default-key.pem"

    host "a.example.com" {
        tls cert="./a.pem" key="./a-key.pem"
        route "/*" {
            files root="./site-a"
        }
    }

    host "b.example.com" {
        tls cert="./b.pem" key="./b-key.pem"
        route "/*" {
            files root="./site-b"
        }
    }
}
```

The same certificate resolver feeds both the rustls acceptor and (with the `h3`
feature) the QUIC/HTTP/3 listener, so per-host certificate selection works
identically on HTTP/1.1, HTTP/2, and HTTP/3.

:::tip Routing and certificates are independent

SNI selects the certificate during the TLS handshake; the `Host` header (or
`:authority`) selects the routes after the connection is established. They
usually name the same host, but they're resolved separately — a binding-level
fallback certificate can still terminate TLS for a request that then falls
through to the default vhost.

:::

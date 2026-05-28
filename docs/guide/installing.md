---
title: Installing
---

# Installing

## From crates.io

```sh
cargo install trillium-cli
```

This installs a single binary named `trillium`. Run `trillium --help`, or
`trillium <command> --help`, for the full option list.

The default build includes the `serve`, `proxy`, `client`, and `bench`
subcommands, with the `rustls` TLS backend and HTTP/3 (`h3`). To build a smaller
binary, or to include a non-default subcommand like `gateway`, select
[features](#feature-flags) explicitly:

```sh
# just the client, built against the system's native TLS
cargo install trillium-cli --no-default-features --features client,native-tls

# everything, including the non-default subcommands
cargo install trillium-cli --features gateway,grpc,dev-server
```

## Feature flags

Each subcommand is gated behind a Cargo feature, so you only compile what you
ship.

| Feature      | Subcommand                  | Default | Notes                                       |
|--------------|-----------------------------|:-------:|---------------------------------------------|
| `serve`      | [`serve`](./serve)          | ✅      | static file server + reverse proxy          |
| `proxy`      | [`proxy`](./proxy)          | ✅      | reverse / forward proxy with caching        |
| `client`     | [`client`](./client)        | ✅      | HTTP client                                 |
| `bench`      | [`bench`](./bench)          | ✅      | load generator                              |
| `gateway`    | [`gateway`](./gateway/overview) |     | config-driven multi-listener server (KDL)   |
| `dev-server` | [`dev-server`](./dev-server)|         | watch / rebuild / restart loop (Unix only)  |
| `grpc`       | [`grpc`](./grpc)            |         | generate Rust modules from `.proto` files   |

The non-default features (`gateway`, `dev-server`, `grpc`) are off because they
pull in heavier dependencies — a KDL parser, a file watcher, the protobuf
toolchain — that the common server/client/bench workflow doesn't need.

## TLS backends

The TLS backend is also selectable. Pick exactly one as the default for a build:

| Feature       | Default | Notes                                                       |
|---------------|:-------:|-------------------------------------------------------------|
| `rustls`      | ✅      | pure-Rust TLS; implied by `h3`                              |
| `native-tls`  |         | the platform's native TLS (Secure Transport, SChannel, …)   |
| `openssl`     |         | OpenSSL                                                      |
| `h3`          | ✅      | HTTP/3 over QUIC; implies `rustls`                          |

When several backends are compiled in, the `--tls` / `--client-tls` flags choose
between them at runtime; the default precedence is `rustls` → `native-tls` →
`openssl`. The `gateway` subcommand always uses `rustls` (and `h3` when
enabled).

```sh
# server + client with OpenSSL instead of rustls, no HTTP/3
cargo install trillium-cli --no-default-features --features serve,client,openssl
```

## Building from source

```sh
git clone https://github.com/trillium-rs/trillium-cli
cd trillium-cli
cargo build --release        # release builds use fat LTO
```

The same `--features` / `--no-default-features` selection applies:

```sh
cargo build --release --features gateway
cargo run -- serve ./public          # run a subcommand from the checkout
cargo run --features gateway -- gateway --config gateway.kdl
```

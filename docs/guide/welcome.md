---
title: Welcome
slug: /
---

# trillium-cli

A single `trillium` binary that bundles the most useful pieces of the
[trillium.rs](https://trillium.rs) web stack into a batteries-included HTTP
toolkit:

- **[`serve`](./serve)** — a static file server (and drop-in reverse proxy)
- **[`proxy`](./proxy)** — a reverse / forward proxy with upstream
  load-balancing and caching
- **[`gateway`](./gateway/overview)** — a config-driven server combining static
  files + proxy across one or more listeners
- **[`client`](./client)** — a curl-like HTTP client that pretty-prints JSON and
  follows redirects
- **[`bench`](./bench)** — a load generator with HDR-histogram latency statistics
- **[`dev-server`](./dev-server)** — a watch / rebuild / restart loop with
  browser live-reload (Unix only)
- **[`grpc`](./grpc)** — generate Rust modules from `.proto` service definitions

TLS is built in (rustls by default), and with the default `h3` feature the
servers also speak HTTP/3 over QUIC. Over TLS the `client` negotiates HTTP/2
via ALPN; `--http-version` selects the protocol (HTTP/1.0 through HTTP/3) for
`client` and `bench`.

## Install

The quickest path is a prebuilt binary — every release ships them for macOS,
Linux, and Windows, with every subcommand already enabled:

```sh
# with cargo-binstall:
cargo binstall trillium-cli

# or the platform installer (macOS/Linux):
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/trillium-rs/trillium-cli/releases/latest/download/trillium-cli-installer.sh | sh
```

Or compile from source with cargo:

```sh
cargo install trillium-cli
```

This installs a binary named `trillium`. Run `trillium --help`, or
`trillium <command> --help`, for the full option list — these guide pages
cover the common cases and call out the surface that isn't immediately obvious
from `--help`.

With `cargo install`, each subcommand is gated behind a Cargo feature and the
TLS backend is selectable, so you can build a smaller binary with only what you
need (and the non-default `gateway`, `dev-server`, and `grpc` subcommands are
opt-in). See [Installing](./installing) for the prebuilt-binary details, the
feature matrix, and building from source.

## Environment variables

Most listening and connection options also read from environment variables —
`HOST`, `PORT`, `CERT`, `KEY`, `FORWARD`, `UPSTREAM` — so flags compose well
with `.env` files and process managers. Each subcommand page lists the
specific env vars it honors.

## License

Licensed under either of [MIT](https://github.com/trillium-rs/trillium-cli/blob/main/LICENSE-MIT)
or [Apache-2.0](https://github.com/trillium-rs/trillium-cli/blob/main/LICENSE-APACHE)
at your option.

---
title: Installing
---

import Install from '@site/src/components/Install';

# Installing

The CLI installs a single binary named `trillium`. Run `trillium --help`, or
`trillium <command> --help`, for the full option list.

## Prebuilt binaries (recommended)

Every release ships prebuilt binaries for x86_64/aarch64 macOS, x86_64 Linux,
and x86_64 Windows — no Rust toolchain and no compilation required. They're
built with **every subcommand enabled** (including the otherwise-opt-in
`gateway`, `grpc`, and, on macOS/Linux, `dev-server`), so the full toolkit is
available out of the box.

<Install />

The methods below spell out each option; the picker above just jumps to the one
for your platform.

### cargo-binstall

If you have [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), it
reads the release metadata and downloads the right binary for your platform:

```sh
cargo binstall trillium-cli
```

### Installer scripts

The installer scripts detect your platform, download the matching archive, and
place the binary in `~/.cargo/bin`.

**macOS and Linux:**

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/trillium-rs/trillium-cli/releases/latest/download/trillium-cli-installer.sh | sh
```

**Windows (PowerShell):**

```powershell
powershell -c "irm https://github.com/trillium-rs/trillium-cli/releases/latest/download/trillium-cli-installer.ps1 | iex"
```

Both always install the most recent release.

### Download the archive directly

Prefer to fetch and unpack the archive yourself, or pin a specific version? Grab
the platform archive (`.tar.xz` on macOS/Linux, `.zip` on Windows) from the
[releases page](https://github.com/trillium-rs/trillium-cli/releases), verify it
against the published `sha256.sum`, and put the `trillium` binary somewhere on
your `PATH`.

## From crates.io

To compile from source with cargo:

```sh
cargo install trillium-cli
```

A default `cargo install` includes the `serve`, `proxy`, `client`, and `bench`
subcommands, with the `rustls` TLS backend and HTTP/3 (`h3`) — the non-default
`gateway`, `grpc`, and `dev-server` subcommands are off (the prebuilt binaries
above bundle them all). To build a smaller binary, or to add a non-default
subcommand, select [features](#feature-flags) explicitly:

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

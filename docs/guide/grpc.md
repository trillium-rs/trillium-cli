---
title: grpc
---

# `trillium grpc`

Generate Rust modules from a `.proto` service definition. This is a build-time
codegen tool, not a server — it turns protobuf service definitions into
committed Rust source you wire into a trillium app.

:::note Feature-gated

`grpc` is **not** in the default build. Install it with
`cargo install trillium-cli --features grpc`, or run from a checkout with
`cargo run --features grpc -- grpc`.

:::

## Usage

```sh
trillium grpc <PROTO> [OUT]
```

```sh
trillium grpc ./proto/echo.proto            # writes into ./src
trillium grpc ./proto/echo.proto ./src/gen  # writes into ./src/gen
```

`grpc` produces one `.rs` file per `.proto` package, written into the output
directory (default `./src`, created if missing). Each generated file contains:

- the [`prost`](https://docs.rs/prost)-generated message types,
- the `trillium-grpc` service trait, and
- a `Server<T>` Handler you can mount into a trillium app.

Output is formatted with [`prettyplease`](https://docs.rs/prettyplease) and is
intended to be committed to your repository (rather than regenerated in a
`build.rs`).

## Imports

The directory containing the `.proto` is added to the include path
automatically. Add more include paths with `-I` / `--include` when your
definitions `import` from elsewhere:

```sh
trillium grpc ./proto/api.proto -I ./proto/common -I ./vendor/googleapis
```

## Generating only the client or server

By default `grpc` emits both halves of each service: the `<Service>Client` struct
with its call methods, and the service trait plus the `<Service>Server<T>`
handler. Pass `--emit` to generate just the half you need:

```sh
trillium grpc ./proto/echo.proto --emit client   # only the calling side
trillium grpc ./proto/echo.proto --emit server   # only the implementing side
```

| Value    | Generates                                                         |
|----------|------------------------------------------------------------------|
| `both`   | client and server (the default)                                  |
| `client` | only the `<Service>Client` struct and its call methods           |
| `server` | only the service trait and the `<Service>Server<T>` handler      |

Use `client` for a crate that only calls the service and `server` for one that
only implements it, so each side avoids compiling code it never uses.

## Full flag reference

```
trillium grpc [OPTIONS] <PROTO> [OUT]

Arguments:
  <PROTO>  Path to the .proto file to compile
  [OUT]    Output directory for generated .rs files  [default: ./src]

Options:
  -I, --include <INCLUDES>  Additional include path for resolving imports
      --emit <EMIT>         Which halves to generate: both, client, server  [default: both]
  -h, --help
```

---
title: client
---

# `trillium client`

A curl-like HTTP client that pretty-prints JSON, streams bodies, follows
redirects by default, and negotiates HTTP/2 over TLS via ALPN.

```sh
trillium client <METHOD> <URL> [OPTIONS]
```

```sh
trillium client get https://example.com
trillium client get https://httpbin.org/json
```

The method is case-insensitive (`get`, `GET`, `Get` all work). A URL with no
scheme is assumed to be `http://`.

## Output

When stdout is a terminal, `client` formats for humans:

- **JSON** responses (any `application/json` or `+json` content type) are
  pretty-printed with syntax-colored output.
- Other bodies stream straight to stdout.
- A colored `Status:` line is printed for non-`200` responses.

When stdout is **not** a terminal (piped or redirected), the colorized
status line and request log are suppressed and JSON is emitted as plain
pretty-printed text, so it composes cleanly with `jq` and friends:

```sh
trillium client get https://httpbin.org/json | jq .slideshow.title
```

Raise the verbosity with `-v` to also print the request line, request and
response headers, peer address, negotiated version, and any response trailers.

## Request bodies

Provide a body inline, from a file, or from stdin — the three forms are
equivalent:

```sh
trillium client post https://httpbin.org/anything -b '{"hello": "world"}'
trillium client post https://httpbin.org/anything -f ./body.json
cat ./body.json | trillium client post https://httpbin.org/anything
```

| Flag           | Notes                                            |
|----------------|--------------------------------------------------|
| `-b`, `--body` | inline request body string                       |
| `-f`, `--file` | read the body from a file (streamed)             |
| _(stdin)_      | a piped/redirected stdin is streamed as the body |

File and stdin bodies are streamed, not buffered, so they're fine for large
uploads.

## Headers

Repeat `-H KEY=VALUE` for each header:

```sh
trillium client post https://httpbin.org/anything \
  -H Authorization="Bearer $TOKEN" -H Content-Type=application/json \
  -b '{"hello": "world"}'
```

## Saving the body

`-o` / `--output-file` writes the response body to a file instead of stdout.
With no argument it derives the filename from the URL's last path segment:

```sh
trillium client get https://example.com/report.pdf -o          # → ./report.pdf
trillium client get https://example.com/report.pdf -o out.pdf  # → ./out.pdf
```

## Redirects

Redirects are followed automatically (up to 10 hops). HTTPS→HTTP downgrades are
refused unless you opt in.

| Flag                     | Default | Notes                                              |
|--------------------------|---------|----------------------------------------------------|
| `--no-follow-redirects`  |         | print the 3xx response as-is instead of following  |
| `--max-redirects`        | `10`    | maximum hops before erroring                       |
| `--allow-downgrade`      |         | permit following an `https://` → `http://` redirect|

## TLS and HTTP version

```sh
trillium client get https://example.com --http-version 2
trillium client get https://localhost:8443 -k     # self-signed dev cert
```

| Flag               | Default  | Notes                                                       |
|--------------------|----------|-------------------------------------------------------------|
| `-t`, `--tls`      | `rustls` | client TLS backend: `rustls`/`native`/`openssl`/`none`      |
| `--http-version`   | `1.1`    | `0.9`, `1.0`, `1.1`, `2`, or `3` (h3 requires the `h3` feature) |
| `-k`, `--insecure` |          | skip certificate verification (rustls only) — **dangerous** |

Over TLS, HTTP/2 is negotiated via ALPN when the server supports it; pass
`--http-version` to force a specific protocol. Requests to `https://` URLs with
`--tls none` will fail.

## Timeouts and dry runs

```sh
trillium client get https://slow.example.com --timeout 30s
trillium client post https://api.example.com -b '{}' --dry-run
```

| Flag           | Default | Notes                                             |
|----------------|---------|---------------------------------------------------|
| `--timeout`    | `10s`   | per-request timeout (e.g. `30s`, `1m`, `500ms`)   |
| `--no-timeout` |         | disable the per-request timeout entirely          |
| `--dry-run`    |         | print the request that would be sent, then exit   |

`--dry-run` is handy for inspecting exactly what would go over the wire —
method, URL, headers, and body — without making a request.

## Full flag reference

```
trillium client [OPTIONS] <METHOD> <URL>

Arguments:
  <METHOD>  HTTP method (case-insensitive)
  <URL>     Request URL (http:// assumed if no scheme)

Options:
  -b, --body <BODY>
  -f, --file <FILE>
  -o, --output-file [<OUTPUT_FILE>]
  -H, --headers <HEADERS>          KEY=VALUE, repeatable
  -t, --tls <TLS>                  [default: rustls]
      --http-version <HTTP_VERSION>  [default: 1.1]
  -k, --insecure
      --dry-run
  -v, --verbose...
  -q, --quiet...
  -h, --help

Timeout:
      --timeout <TIMEOUT>          [default: 10s]
      --no-timeout

Redirects:
      --no-follow-redirects
      --max-redirects <MAX_REDIRECTS>  [default: 10]
      --allow-downgrade
```

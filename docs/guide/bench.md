---
title: bench
---

# `trillium bench`

A load generator that reports latency as an HDR-histogram summary. It runs in
two modes: **closed-loop** (a fixed pool of connections, each firing the next
request as soon as the previous completes) and **open-loop** (requests
scheduled at a target arrival rate, independent of how fast they complete).

```sh
trillium bench <URL> [OPTIONS]
```

## Closed-loop (default)

By default, `bench` opens 50 concurrent connections and runs for 10 seconds:

```sh
trillium bench https://localhost:8080
```

Tune the concurrency and stopping condition:

```sh
trillium bench https://localhost:8080 -c 200 -d 30s     # 200 connections for 30s
trillium bench https://localhost:8080 -c 100 -n 1000000 # 100 connections, 1M requests total
```

| Flag                  | Default | Notes                                              |
|-----------------------|---------|----------------------------------------------------|
| `-c`, `--connections` | `50`    | concurrent connections                             |
| `-d`, `--duration`    | `10s`   | run for this long (e.g. `10s`, `1m`, `30s500ms`)   |
| `-n`, `--requests`    |         | stop after this many requests (closed-loop only)   |

`--duration` and `--requests` are mutually exclusive; with neither, `bench`
runs for 10 seconds.

## Open-loop

Passing `-r` / `--rate` switches to open-loop scheduling: requests are launched
at a fixed offered rate (requests per second) regardless of how quickly the
server responds. This is the mode for measuring latency under a known load,
since it doesn't let a slow server throttle the offered rate (avoiding
"coordinated omission").

```sh
trillium bench https://localhost:8080 --rate 5000 --duration 30s
trillium bench https://localhost:8080 --rate 5000 --pacing poisson
```

| Flag                | Default   | Notes                                                       |
|---------------------|-----------|-------------------------------------------------------------|
| `-r`, `--rate`      |           | target arrival rate (req/s); enables open-loop              |
| `--pacing`          | `uniform` | `uniform` (fixed interval) or `poisson` (exponential gaps)  |
| `--max-concurrency` |           | hard cap on in-flight requests; excess are dropped as saturation |

When the server can't keep up with the offered rate, scheduled requests that
would exceed `--max-concurrency` are counted as **saturation drops** in the
report — a direct signal that you've found the server's ceiling.

## Request shape

`bench` shares the [`client`](./client) flags for method, headers, body, TLS,
and HTTP version:

```sh
trillium bench https://api.example.com/items -m POST \
  -H Content-Type=application/json -b '{"q":"test"}'

trillium bench https://api.example.com/upload --body-size 4kb   # synthetic body
```

| Flag             | Default | Notes                                               |
|------------------|---------|-----------------------------------------------------|
| `-m`, `--method` | `GET`   | HTTP method                                         |
| `-H`, `--headers`|         | `KEY=VALUE`, repeatable                             |
| `-f`, `--file`   |         | request body from a file                            |
| `-b`, `--body`   |         | inline request body                                 |
| `--body-size`    |         | synthesize a zero-filled body of this size (`4kb`, `1mb`) |
| `--http-version` | `1.1`   | `0.9`–`3`                                           |
| `-t`, `--tls`    | `rustls`| TLS backend                                         |
| `--no-keepalive` |         | disable HTTP/1.1 connection reuse                   |

## Warmup and timeout

```sh
trillium bench https://localhost:8080 -d 1m --warmup 5s --timeout 2s
```

| Flag            | Notes                                                       |
|-----------------|-------------------------------------------------------------|
| `-w`, `--warmup`| discard statistics collected during this initial period     |
| `--timeout`     | per-request timeout                                         |

`--warmup` lets connection pools and JITs settle before measurement begins, so
the histogram reflects steady state rather than cold-start latency.

## Reading the report

When stdout is a terminal, `bench` shows a live progress bar during the run and
then prints a report with these sections:

- **Summary** — elapsed time, completed/succeeded counts, request throughput
  (req/s), and bytes sent/received with receive throughput.
- **Status codes** — a count per HTTP status, colored by class.
- **Errors** — counts bucketed into `io`, `timeout`, `protocol`, `other`, plus
  `saturation drops` (open-loop).
- **Latency (full response)** and **Latency (TTFB)** — HDR-histogram
  percentiles (`min`, `mean`, `p50`, `p75`, `p90`, `p95`, `p99`, `p99.9`,
  `max`, `stdev`). TTFB is time-to-first-byte.
- **Open-loop queue wait** — in open-loop mode, how long scheduled requests
  waited for a free slot (a second saturation signal).

## Machine-readable output

```sh
trillium bench https://localhost:8080 --json > report.json
trillium bench https://localhost:8080 --csv timings.csv
```

| Flag            | Notes                                                          |
|-----------------|----------------------------------------------------------------|
| `--json`        | emit the full report as JSON to stdout (suppresses the bar)    |
| `--csv <PATH>`  | write per-request timing samples (scheduled/started offsets, queue, TTFB, total, status, bytes) to a CSV file |
| `--no-progress` | suppress the live progress display even on a tty               |

The CSV captures one row per request, suitable for plotting latency over time
or post-hoc percentile analysis.

## Tuning the client's HTTP layer

For squeezing the client side, `bench` exposes a few
[`trillium_http::HttpConfig`](https://docs.rs/trillium-http) knobs. These are
rarely needed; reach for them only when the client itself is the bottleneck.

```
--response-buffer-len <BYTES>
--response-buffer-max-len <BYTES>
--head-max-len <BYTES>
--copy-loops-per-yield <N>
--received-body-max-len <BYTES>
```

## Full flag reference

```
trillium bench [OPTIONS] <URL>

Options:
  -m, --method <METHOD>            [default: GET]
  -c, --connections <CONNECTIONS>  [default: 50]
  -d, --duration <DURATION>        (conflicts with --requests)
  -n, --requests <REQUESTS>        (conflicts with --duration)
  -r, --rate <RATE>                target req/s; switches to open-loop
      --pacing <PACING>            [default: uniform]  (uniform | poisson)
      --max-concurrency <N>
  -w, --warmup <WARMUP>
      --timeout <TIMEOUT>
  -H, --headers <HEADERS>          KEY=VALUE, repeatable
  -f, --file <FILE>
  -b, --body <BODY>
      --body-size <BODY_SIZE>
      --http-version <HTTP_VERSION>  [default: 1.1]
  -t, --tls <TLS>                  [default: rustls]
      --no-keepalive
      --json
      --csv <CSV>
      --no-progress
  -v, --verbose...
  -q, --quiet...
  -h, --help
```

```
$ trillium help
The trillium.rs cli

Usage: trillium <COMMAND>

Commands:
  serve   Static file server and reverse proxy
  client  Make http requests using the trillium client
  bench   Generate http load and report latency/throughput statistics
  proxy   Run a http proxy
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

# HTTP Client

```
$ trillium help client
Make http requests using the trillium client

Usage: trillium client [OPTIONS] <METHOD> <URL>

Arguments:
  <METHOD>
          

  <URL>
          

Options:
  -f, --file <FILE>
          provide a file system path to a file to use as the request body
          
          alternatively, you can use an operating system pipe to pass a file in
          
          three equivalent examples:
          
          trillium client post http://httpbin.org/anything -f ./body.json
          trillium client post http://httpbin.org/anything < ./body.json
          cat ./body.json | trillium client post http://httpbin.org/anything

  -o, --output-file [<OUTPUT_FILE>]
          write the body to a file

  -b, --body <BODY>
          provide a request body on the command line
          
          example:
          trillium client post http://httpbin.org/post -b '{"hello": "world"}'

  -H, --headers <HEADERS>
          provide headers in the form -h KEY1=VALUE1 KEY2=VALUE2
          
          example:
          trillium client get http://httpbin.org/headers -H Accept=application/json Authorization="Basic u:p"

  -t, --tls <TLS>
          tls implementation
          
          requests to https:// urls with `none` will fail
          
          [default: rustls]
          [possible values: none, rustls]

      --http-version <HTTP_VERSION>
          http version

          Possible values:
          - 0.9: HTTP/0.9
          - 1.0: HTTP/1.0
          - 1.1: HTTP/1.1
          - 2:   HTTP/2
          - 3:   HTTP/3
          
          [default: 1.1]

  -v, --verbose...
          Increase logging verbosity

  -q, --quiet...
          Decrease logging verbosity

  -h, --help
          Print help (see a summary with '-h')
```
# Proxy (reverse and forward)

```
$ trillium help proxy
Run a http proxy

Usage: trillium proxy [OPTIONS] [UPSTREAM]...

Arguments:
  [UPSTREAM]...
          [env: UPSTREAM=]

Options:
  -s, --strategy <STRATEGY>
          [env: STRATEGY=]
          [default: round-robin]
          [possible values: round-robin, connection-counting, random, forward]

  -o, --host <HOST>
          Local host or ip to listen on
          
          [env: HOST=]
          [default: localhost]

  -p, --port <PORT>
          Local port to listen on
          
          [env: PORT=]
          [default: 8080]

      --cert <CERT>
          Path to a tls certificate file
          
          This will fail unless key is also provided. Providing both cert and key enables tls.
          
          Example: `--cert ./cert.pem --key ./key.pem` For development, try using mkcert or rcgen
          
          [env: CERT=]

      --key <KEY>
          The path to a tls key file
          
          This will fail unless cert is also provided. Providing both cert and key enables tls.
          
          Example: `--cert ./cert.pem --key ./key.pem` For development, try using mkcert or rcgen
          
          [env: KEY=]

      --tls <TLS>
          [env: TLS=]
          [default: rustls]
          [possible values: none, rustls]

  -c, --client-tls <CLIENT_TLS>
          tls implementation
          
          required if the upstream url is https.
          
          [default: rustls]
          [possible values: none, rustls]

  -v, --verbose...
          Increase logging verbosity

  -q, --quiet...
          Decrease logging verbosity

  -h, --help
          Print help (see a summary with '-h')
```

# Static file server

```
$ trillium help serve
Static file server and reverse proxy

Usage: trillium serve [OPTIONS] [ROOT]

Arguments:
  [ROOT]
          Filesystem path to serve
          
          Defaults to the current working directory
          
          [default: {your working directory}]

Options:
  -o, --host <HOST>
          Local host or ip to listen on
          
          [env: HOST=]
          [default: localhost]

  -p, --port <PORT>
          Local port to listen on
          
          [env: PORT=]
          [default: 8080]

      --cert <CERT>
          Path to a tls certificate file
          
          This will fail unless key is also provided. Providing both cert and key enables tls.
          
          Example: `--cert ./cert.pem --key ./key.pem` For development, try using mkcert or rcgen
          
          [env: CERT=]

      --key <KEY>
          The path to a tls key file
          
          This will fail unless cert is also provided. Providing both cert and key enables tls.
          
          Example: `--cert ./cert.pem --key ./key.pem` For development, try using mkcert or rcgen
          
          [env: KEY=]

      --tls <TLS>
          [env: TLS=]
          [default: rustls]
          [possible values: none, rustls]

  -f, --forward <FORWARD>
          Host to forward (reverse proxy) not-found requests to
          
          This forwards any request that would otherwise be a 404 Not Found to the specified listener spec.
          
          Examples: `--forward localhost:8081` `--forward http://localhost:8081` `--forward https://localhost:8081`
          
          Note: http+unix:// schemes are not yet supported
          
          [env: FORWARD=]

  -i, --index <INDEX>
          [env: INDEX=]

  -v, --verbose...
          Increase logging verbosity

  -q, --quiet...
          Decrease logging verbosity

  -h, --help
          Print help (see a summary with '-h')
```

# Load generation / testing

```
$ trillium help bench
Generate http load and report latency/throughput statistics

Usage: trillium bench [OPTIONS] <URL>

Arguments:
  <URL>
          target URL to benchmark

Options:
  -m, --method <METHOD>
          HTTP method
          
          [default: GET]

  -c, --connections <CONNECTIONS>
          number of concurrent connections (closed-loop) or initial pool size (open-loop)
          
          [default: 50]

  -d, --duration <DURATION>
          total test duration (e.g. 10s, 1m, 30s500ms)
          
          mutually exclusive with --requests; default is 10s when neither is specified.

  -n, --requests <REQUESTS>
          total number of requests to send (closed-loop only)

  -r, --rate <RATE>
          target rate in requests per second; switches to open-loop scheduling

      --pacing <PACING>
          open-loop pacing strategy

          Possible values:
          - uniform: fixed inter-arrival interval = 1 / rate
          - poisson: exponentially-distributed inter-arrival times with mean 1 / rate
          
          [default: uniform]

      --max-concurrency <MAX_CONCURRENCY>
          in open-loop mode, hard cap on simultaneous in-flight requests
          
          scheduled tickets that would exceed this cap are dropped and counted as saturation.

  -w, --warmup <WARMUP>
          discard statistics collected during this initial period

      --timeout <TIMEOUT>
          per-request timeout

  -H, --headers <HEADERS>
          request headers in KEY=VALUE form, repeatable

  -f, --file <FILE>
          path to a file to use as the request body

  -b, --body <BODY>
          inline request body string

      --body-size <BODY_SIZE>
          synthesize a zero-filled request body of the given size (e.g. 4kb, 1mb)

      --http-version <HTTP_VERSION>
          http version

          Possible values:
          - 0.9: HTTP/0.9
          - 1.0: HTTP/1.0
          - 1.1: HTTP/1.1
          - 2:   HTTP/2
          - 3:   HTTP/3
          
          [default: 1.1]

  -t, --tls <TLS>
          tls implementation
          
          [default: rustls]
          [possible values: none, rustls]

      --no-keepalive
          disable http/1.1 connection reuse

      --response-buffer-len <RESPONSE_BUFFER_LEN>
          HttpConfig: initial response buffer length (bytes)

      --response-buffer-max-len <RESPONSE_BUFFER_MAX_LEN>
          HttpConfig: maximum response buffer length under backpressure (bytes)

      --head-max-len <HEAD_MAX_LEN>
          HttpConfig: max length of the http head (request line + headers)

      --copy-loops-per-yield <COPY_LOOPS_PER_YIELD>
          HttpConfig: cooperative yield interval for the copy loop

      --received-body-max-len <RECEIVED_BODY_MAX_LEN>
          HttpConfig: maximum allowed received body length (bytes)

      --json
          emit the final report as JSON to stdout

      --csv <CSV>
          write per-request timing data as CSV to this path

      --no-progress
          suppress the live progress display even when stdout is a tty

  -v, --verbose...
          Increase logging verbosity

  -q, --quiet...
          Decrease logging verbosity

  -h, --help
          Print help (see a summary with '-h')
```

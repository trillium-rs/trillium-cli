```
$ trillium help
The trillium.rs cli

Usage: trillium <COMMAND>

Commands:
  serve   Static file server and reverse proxy
  client  Make http requests using the trillium client
  proxy   Run a http proxy
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version

```

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

  -v, --verbose...
          Increase logging verbosity

  -q, --quiet...
          Decrease logging verbosity

  -h, --help
          Print help (see a summary with '-h')
```

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

      --rustls-cert <RUSTLS_CERT>
          Path to a tls certificate for trillium_rustls
          
          This will panic unless rustls_key is also provided. Providing both rustls_key and rustls_cert enables tls.
          
          Example: `--rustls-cert ./cert.pem --rustls-key ./key.pem` For development, try using mkcert
          
          [env: RUSTLS_CERT=]

      --rustls-key <RUSTLS_KEY>
          The path to a tls key file for trillium_rustls
          
          This will panic unless rustls_cert is also provided. Providing both rustls_key and rustls_cert enables tls.
          
          Example: `--rustls-cert ./cert.pem --rustls-key ./key.pem` For development, try using mkcert
          
          [env: RUSTLS_KEY=]

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

```
$ trillium help serve
Static file server and reverse proxy

Usage: trillium serve [OPTIONS] [ROOT]

Arguments:
  [ROOT]
          Filesystem path to serve
          
          Defaults to the current working directory
          
          [default: /Users/jbr/code/futures-rustls]

Options:
  -o, --host <HOST>
          Local host or ip to listen on
          
          [env: HOST=]
          [default: localhost]

  -p, --port <PORT>
          Local port to listen on
          
          [env: PORT=]
          [default: 8080]

      --rustls-cert <RUSTLS_CERT>
          Path to a tls certificate for trillium_rustls
          
          This will panic unless rustls_key is also provided. Providing both rustls_key and rustls_cert enables tls.
          
          Example: `--rustls-cert ./cert.pem --rustls-key ./key.pem` For development, try using mkcert
          
          [env: RUSTLS_CERT=]

      --rustls-key <RUSTLS_KEY>
          The path to a tls key file for trillium_rustls
          
          This will panic unless rustls_cert is also provided. Providing both rustls_key and rustls_cert enables tls.
          
          Example: `--rustls-cert ./cert.pem --rustls-key ./key.pem` For development, try using mkcert
          
          [env: RUSTLS_KEY=]

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

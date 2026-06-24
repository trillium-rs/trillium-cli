use crate::tls::{Tls, parse_url};
use blocking::Unblock;
use clap::Parser;
use clap_verbosity_flag::Verbosity;
use colored::*;
use log::Level;
use std::{
    io::{ErrorKind, IsTerminal},
    path::PathBuf,
    time::Duration,
};
use trillium_client::{
    Body, Client, Conn, Error, Headers, KnownHeaderName, Method, Status, Url, Version,
};
use trillium_client_retry::RetryHandler;
use trillium_compression::{CompressionAlgorithm, client::Compression};
use trillium_logger::client::ClientLogger;
use trillium_redirect::client::FollowRedirects;

#[derive(Parser, Debug)]
pub struct ClientCli {
    #[arg(value_parser = parse_method_case_insensitive)]
    method: Method,

    #[arg(value_parser = parse_url)]
    url: Url,

    /// provide a file system path to a file to use as the request body
    ///
    /// alternatively, you can use an operating system pipe to pass a file in
    ///
    /// three equivalent examples:
    ///
    /// trillium client post http://httpbin.org/anything -f ./body.json
    /// trillium client post http://httpbin.org/anything < ./body.json
    /// cat ./body.json | trillium client post http://httpbin.org/anything
    #[arg(short, long, verbatim_doc_comment)]
    file: Option<PathBuf>,

    /// write the body to a file
    #[arg(short, long, verbatim_doc_comment)]
    output_file: Option<Option<PathBuf>>,

    /// provide a request body on the command line
    ///
    /// example:
    /// trillium client post http://httpbin.org/post -b '{"hello": "world"}'
    #[arg(short, long, verbatim_doc_comment)]
    body: Option<String>,

    /// provide headers in the form -h KEY1=VALUE1 KEY2=VALUE2
    ///
    /// example:
    /// trillium client get http://httpbin.org/headers -H Accept=application/json Authorization="Basic u:p"
    #[arg(short = 'H', long, value_parser = parse_header, verbatim_doc_comment)]
    headers: Vec<(String, String)>,

    /// compress the request body with this encoding
    ///
    /// responses are always decoded transparently regardless of this flag; it
    /// only controls the outbound body. there is no negotiation for request
    /// bodies, so only use this against an origin known to accept it.
    #[arg(short = 'c', long, verbatim_doc_comment, value_enum)]
    compression: Option<RequestEncoding>,

    /// tls implementation
    ///
    /// requests to https:// urls with `none` will fail
    #[arg(short, long, verbatim_doc_comment, value_enum, default_value_t)]
    tls: Tls,

    /// http version
    #[arg(long, verbatim_doc_comment, value_enum, default_value_t)]
    http_version: HttpVersion,

    /// route DNS through an encrypted resolver instead of the system resolver
    ///
    /// the scheme selects the transport (following the dnsproxy convention):
    ///
    ///   --dns 1.1.1.1                DNS-over-HTTPS, expands to
    ///                                https://1.1.1.1/dns-query
    ///   --dns https://h/dns-query    DNS-over-HTTPS at an explicit url
    ///   --dns tls://1.1.1.1          DNS-over-TLS    (needs a tls backend)
    ///   --dns quic://1.1.1.1         DNS-over-QUIC   (needs --tls rustls + h3)
    ///   --dns h3://1.1.1.1           DNS-over-HTTPS forced over HTTP/3
    ///
    /// a bare host or one given with tls://, quic:// or h3:// expands to the
    /// transport's default port and path; pass a full url to override either.
    ///
    /// beyond encryption, a non-system resolver also fetches SVCB/HTTPS records
    /// (RFC 9460), so a host that advertises alpn=h3 in DNS is reached over
    /// HTTP/3 on the very first request — with no Alt-Svc round-trip — when the
    /// client is http/3-capable (--tls rustls with the h3 build).
    ///
    /// resolution is fail-closed: once set, a lookup the resolver can't answer
    /// fails the request rather than falling back to the system resolver, so a
    /// query never leaks to the local resolver.
    ///
    /// every transport runs over tls, so this needs a tls backend — it has no
    /// effect with --tls none.
    #[cfg(any(feature = "rustls", feature = "native-tls", feature = "openssl"))]
    #[arg(long, value_parser = crate::dns::parse_dns, verbatim_doc_comment, help_heading = "DNS")]
    dns: Option<crate::dns::DnsResolver>,

    /// dial this unix domain socket instead of opening a tcp connection
    ///
    /// the request url still supplies the request metadata — its path, query,
    /// and `Host` header — but its host and port no longer pick a connection
    /// address; the socket at this path is the address. combine with --tls to
    /// speak https over the socket.
    ///
    /// note: --http-version 3 runs HTTP/3 over QUIC, a UDP transport with no
    /// unix-socket equivalent, so it can't be used over a socket.
    #[cfg(unix)]
    #[cfg_attr(
        any(feature = "rustls", feature = "native-tls", feature = "openssl"),
        arg(
            long,
            value_name = "PATH",
            conflicts_with = "dns",
            verbatim_doc_comment
        )
    )]
    #[cfg_attr(
        not(any(feature = "rustls", feature = "native-tls", feature = "openssl")),
        arg(long, value_name = "PATH", verbatim_doc_comment)
    )]
    unix_socket: Option<PathBuf>,

    /// skip TLS certificate verification (rustls only)
    ///
    /// dangerous: this disables authentication of the server. use only against
    /// hosts you control, e.g. a local server with a self-signed certificate.
    #[arg(short = 'k', long, verbatim_doc_comment)]
    insecure: bool,

    /// print the request that would be sent, then exit without sending it
    #[arg(long)]
    dry_run: bool,

    /// per-request timeout, e.g. 30s, 1m, 500ms
    #[arg(long, value_parser = humantime::parse_duration, default_value = "10s", help_heading = "Timeout")]
    timeout: Duration,

    /// disable the per-request timeout entirely
    #[arg(long, conflicts_with = "timeout", help_heading = "Timeout")]
    no_timeout: bool,

    /// don't follow 3xx redirects; print the redirect response as-is
    #[arg(long, help_heading = "Redirects")]
    no_follow_redirects: bool,

    /// maximum number of redirects to follow before erroring
    #[arg(
        long,
        default_value_t = 10,
        conflicts_with = "no_follow_redirects",
        help_heading = "Redirects"
    )]
    max_redirects: u32,

    /// follow redirects from https to http (blocked by default)
    #[arg(
        long,
        conflicts_with = "no_follow_redirects",
        help_heading = "Redirects"
    )]
    allow_downgrade: bool,

    /// retry failed requests up to this many times (0 disables)
    ///
    /// retries transport errors (connection refused, reset, timeout) and the
    /// retryable statuses 429 and 503, with exponential backoff and honoring a
    /// server-advertised Retry-After. only idempotent methods (GET, HEAD, PUT,
    /// DELETE, OPTIONS, TRACE) are retried unless --retry-all-methods is given.
    ///
    /// note: a request body streamed from stdin or --file cannot be replayed,
    /// so such a request is never retried; use --body for a retryable body.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 0,
        verbatim_doc_comment,
        help_heading = "Retries"
    )]
    retry: u32,

    /// wait a fixed delay between retries instead of exponential backoff
    ///
    /// example: --retry-delay 500ms
    #[arg(
        long,
        value_parser = humantime::parse_duration,
        requires = "retry",
        verbatim_doc_comment,
        help_heading = "Retries"
    )]
    retry_delay: Option<Duration>,

    /// total wall-clock budget across all attempts (default 30s)
    ///
    /// keep this at least as large as --timeout; the first attempt uses the
    /// per-request timeout, and this budget caps every attempt after it.
    #[arg(
        long,
        value_parser = humantime::parse_duration,
        requires = "retry",
        verbatim_doc_comment,
        help_heading = "Retries"
    )]
    retry_max_time: Option<Duration>,

    /// also retry non-idempotent methods such as POST and PATCH
    ///
    /// only safe when the endpoint is idempotent in practice or guarded by an
    /// idempotency key, since replaying it may duplicate a side effect.
    #[arg(
        long,
        requires = "retry",
        verbatim_doc_comment,
        help_heading = "Retries"
    )]
    retry_all_methods: bool,

    /// log one line per request (including each retry attempt) to stderr
    ///
    /// the request logger is shown automatically when stdout is a terminal. when
    /// output is piped or redirected it is suppressed so it can't corrupt the
    /// response body; pass this to force it on, writing to stderr so stdout
    /// stays clean. handy for watching --retry attempts in a script.
    #[arg(long, verbatim_doc_comment)]
    always_log: bool,

    /// set the log level. add more flags for more verbosity
    ///
    /// example:
    /// trillium client get https://www.google.com -vvv # `trace` verbosity level
    #[command(flatten)]
    verbose: Verbosity,
}

#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd, clap::ValueEnum, Default)]
#[non_exhaustive]
pub enum HttpVersion {
    /// HTTP/0.9
    #[value(name = "0.9", alias = "http/0.9", alias = "HTTP/0.9")]
    Http0_9,

    /// HTTP/1.0
    #[value(name = "1.0", alias = "http/1.0", alias = "HTTP/1.0")]
    Http1_0,

    /// HTTP/1.1
    #[value(
        name = "1.1",
        alias = "http/1.1",
        alias = "HTTP/1.1",
        alias = "1",
        alias = "http/1",
        alias = "HTTP/1"
    )]
    #[default]
    Http1_1,

    /// HTTP/2
    #[value(name = "2", alias = "http/2", alias = "HTTP/2")]
    Http2,

    /// HTTP/3
    #[cfg(feature = "h3")]
    #[value(name = "3", alias = "http/3", alias = "HTTP/3")]
    Http3,
}

impl From<HttpVersion> for Version {
    fn from(value: HttpVersion) -> Self {
        match value {
            HttpVersion::Http0_9 => Version::Http0_9,
            HttpVersion::Http1_0 => Version::Http1_0,
            HttpVersion::Http1_1 => Version::Http1_1,
            HttpVersion::Http2 => Version::Http2,
            #[cfg(feature = "h3")]
            HttpVersion::Http3 => Version::Http3,
        }
    }
}

/// Content-codings that can be applied to an outbound request body.
#[derive(Copy, Clone, Debug, Eq, PartialEq, clap::ValueEnum)]
enum RequestEncoding {
    /// Zstandard
    Zstd,

    /// Brotli
    #[value(name = "br", alias = "brotli")]
    Brotli,

    /// gzip
    Gzip,
}

impl From<RequestEncoding> for CompressionAlgorithm {
    fn from(value: RequestEncoding) -> Self {
        match value {
            RequestEncoding::Zstd => CompressionAlgorithm::Zstd,
            RequestEncoding::Brotli => CompressionAlgorithm::Brotli,
            RequestEncoding::Gzip => CompressionAlgorithm::Gzip,
        }
    }
}

impl ClientCli {
    async fn build(&self) -> Conn {
        // On an interactive stdout the logger prints inline (its default
        // `Target::Stdout`). When stdout is piped that would corrupt the body,
        // so the logger is installed only when `--always-log` is given, and
        // then aimed at stderr — `Targetable` is implemented for any
        // `Fn(String)`, so a closure is all it takes — keeping stdout clean.
        // `-v`/`-q` stay dedicated to internal `log` output, not this line.
        let logger = if std::io::stdout().is_terminal() {
            Some(ClientLogger::new())
        } else if self.always_log {
            Some(ClientLogger::new().with_target(|line: String| eprintln!("{line}")))
        } else {
            None
        };

        // `Option<T>` is itself a `ClientHandler`, so a `None` here drops the
        // corresponding step entirely rather than installing a no-op.
        let client_handler = (
            // `--retry 0` (the default) installs no RetryHandler at all. Tuple
            // order is not significant for the retry handler today; the only
            // ordering interaction is with a caching handler, which this client
            // path does not use.
            (self.retry > 0).then(|| {
                let mut handler = RetryHandler::default().with_max_attempts(self.retry + 1);
                if let Some(delay) = self.retry_delay {
                    handler = handler.with_constant_backoff(delay);
                }
                if let Some(max_elapsed) = self.retry_max_time {
                    handler = handler.with_max_elapsed(max_elapsed);
                }
                if self.retry_all_methods {
                    handler = handler.with_all_methods();
                }
                handler
            }),
            Compression::new(),
            logger,
            (!self.no_follow_redirects).then(|| {
                FollowRedirects::new()
                    .with_max_redirects(self.max_redirects)
                    .with_allow_downgrade(self.allow_downgrade)
            }),
        );

        #[cfg(unix)]
        let unix_socket = self.unix_socket.clone();
        #[cfg(not(unix))]
        let unix_socket: Option<PathBuf> = None;

        let client = crate::tls::build_client(self.tls, self.insecure, unix_socket)
            .with_handler(client_handler);
        // `--dns` and `--unix-socket` are mutually exclusive (a fixed socket
        // never resolves a host), so this is a no-op whenever a socket is set.
        let client = self.apply_dns(client);
        let client = if self.no_timeout {
            client.without_timeout()
        } else {
            client.with_timeout(self.timeout)
        };
        let mut conn = client.build_conn(self.method, self.url.clone());
        conn.set_http_version(self.http_version.into());

        conn.request_headers_mut().extend(self.headers.clone());

        if let Some(path) = &self.file {
            let file = async_fs::File::open(path)
                .await
                .unwrap_or_else(|e| panic!("could not read file {:?} ({})", path, e));

            let metadata = file.metadata().await.unwrap();

            conn.set_request_body(Body::new_streaming(file, Some(metadata.len())));
        } else if let Some(body) = &self.body {
            conn.set_request_body(body.clone());
        } else if !std::io::stdin().is_terminal() {
            // stdin is redirected (a pipe, file, or here-string) rather than a
            // terminal. Probe a single byte before committing to a body: an
            // empty source (`</dev/null`, a script with no input) should send
            // no body at all, not an empty chunked request with
            // `Expect: 100-continue`. When there is data, stream the probed
            // byte followed by the rest of stdin — still one-shot, so a request
            // with a piped body is (correctly) not eligible for --retry.
            use std::io::Read;
            let mut probe = [0u8; 1];
            if let Ok(n @ 1..) = std::io::stdin().read(&mut probe) {
                let body = std::io::Cursor::new(probe[..n].to_vec()).chain(std::io::stdin());
                conn.set_request_body(Body::new_streaming(Unblock::new(body), None));
            }
        }

        if let Some(encoding) = self.compression {
            conn.insert_state(CompressionAlgorithm::from(encoding));
        }

        conn
    }

    /// Apply the `--dns` resolver (if any) to `client`, handing the selected
    /// `--tls` to the shared [`crate::dns`] module so it can validate that the
    /// backend can carry the chosen transport.
    #[cfg(any(feature = "rustls", feature = "native-tls", feature = "openssl"))]
    fn apply_dns(&self, client: Client) -> Client {
        match &self.dns {
            Some(dns) => dns.apply(client, self.tls, "--tls"),
            None => client,
        }
    }

    /// In a build with no tls backend the `--dns` flag doesn't exist, so there
    /// is nothing to apply.
    #[cfg(not(any(feature = "rustls", feature = "native-tls", feature = "openssl")))]
    fn apply_dns(&self, client: Client) -> Client {
        client
    }

    /// Print the request line, headers, and (known) body without sending anything.
    fn print_request(&self, conn: &Conn) {
        println!(
            "{} {} {}",
            conn.method().as_str().bold(),
            conn.url().as_str(),
            conn.http_version().as_str().dimmed()
        );
        print_headers(conn.request_headers());

        if let Some(body) = &self.body {
            println!("\n{body}");
        } else if let Some(path) = &self.file {
            println!(
                "\n{}",
                format!("<body streamed from {}>", path.display()).dimmed()
            );
        } else if conn.request_body().is_some() {
            // Only when the stdin probe in `build` actually found data; an empty
            // redirect attaches no body, so there's nothing to note here.
            println!("\n{}", "<body streamed from stdin>".dimmed());
        }
    }

    async fn output_body(&self, conn: &mut Conn) {
        if let Some(file) = &self.output_file {
            let filename = file.clone().unwrap_or_else(|| {
                conn.url()
                    .path_segments()
                    .unwrap()
                    .next_back()
                    .unwrap()
                    .into()
            });
            if filename.to_str().is_none_or(|f| f.is_empty()) {
                eprintln!("specify a filename for this url");
                std::process::exit(-1);
            }
            let bytes_written = futures_lite::io::copy(
                &mut conn.response_body(),
                async_fs::File::create(&filename).await.unwrap(),
            )
            .await
            .unwrap();
            if matches!(self.verbose.log_level(), Some(level) if level >= Level::Warn) {
                println!(
                    "Wrote {} to {}",
                    bytes(bytes_written).italic().bright_blue(),
                    filename.to_string_lossy().italic()
                );
            }
        } else {
            let mime: Option<mime::Mime> = conn
                .response_headers()
                .get_str(KnownHeaderName::ContentType)
                .and_then(|ct| ct.parse().ok());
            let suffix_or_subtype =
                mime.map(|m| m.suffix().unwrap_or_else(|| m.subtype()).to_string());

            match suffix_or_subtype.as_deref() {
                Some("json") => {
                    let body = conn.response_json::<serde_json::Value>().await.unwrap();
                    if std::io::stdout().is_terminal() {
                        println!("{}", colored_json::to_colored_json_auto(&body).unwrap());
                    } else {
                        println!("{}", serde_json::to_string_pretty(&body).unwrap());
                    }
                }

                _ => {
                    match futures_lite::io::copy(
                        &mut conn.response_body(),
                        &mut Unblock::new(std::io::stdout()),
                    )
                    .await
                    {
                        Err(e) if e.kind() == ErrorKind::WriteZero => {}
                        other => {
                            other.unwrap();
                        }
                    }
                }
            }
        }
    }

    pub fn run(self) {
        let runtime = trillium_smol::SmolRuntime::default();

        runtime.block_on(async move {
            env_logger::Builder::new()
                .parse_filters(&format!(
                    "{},quinn=off,quinn_proto=off,rustls=off,tracing=off",
                    self.verbose.log_level_filter()
                ))
                .format(|buf, record| {
                    use std::io::Write;
                    writeln!(
                        buf,
                        "[{} {}] {}",
                        record.module_path().unwrap_or_default().dimmed(),
                        record.level().as_str().dimmed(),
                        record.args()
                    )
                })
                .init();

            let mut conn = self.build().await;

            if self.dry_run {
                use trillium_client::ClientHandler;
                let _ = conn.client().clone().handler().run(&mut conn).await;
                self.print_request(&conn);
                return;
            }

            if let Err(e) = (&mut conn).await {
                match e {
                    Error::Io(io) if io.kind() == ErrorKind::ConnectionRefused => {
                        log::error!("could not reach {}", self.url)
                    }

                    _ => log::error!("protocol error:\n\n{}", e),
                }

                return;
            }

            if std::io::stdout().is_terminal() {
                let status = conn.status().unwrap_or(Status::NotFound);
                if status != Status::Ok
                    || matches!(self.verbose.log_level(), Some(level) if level >= Level::Warn)
                {
                    println!(
                        "{}: {}",
                        "Status".italic().bright_blue(),
                        if status.is_client_error() {
                            status.to_string().yellow()
                        } else if status.is_server_error() {
                            status.to_string().bright_red()
                        } else {
                            status.to_string().green()
                        }
                        .bold()
                    );
                }

                match self.verbose.log_level() {
                    Some(level) if level >= Level::Warn => {
                        println!("{}: {}", "Url".italic().bright_blue(), conn.url().as_str());
                        println!(
                            "{}: {}",
                            "Version".italic().bright_blue(),
                            conn.http_version().as_str().bold()
                        );
                        println!(
                            "{}: {}",
                            "Method".italic().bright_blue(),
                            conn.method().as_str().bold()
                        );
                        if let Some(peer_addr) = conn.peer_addr() {
                            println!("{}: {}", "Peer Address".italic().bright_blue(), peer_addr);
                        }
                        println!("\n{}", "Request Headers".bold().underline());
                        print_headers(conn.request_headers());
                        println!("\n{}", "Response Headers".bold().underline());
                        print_headers(conn.response_headers());

                        println!("\n{}", "Body".bold().underline());
                    }
                    _ => {}
                }
            }

            self.output_body(&mut conn).await;

            if std::io::stdout().is_terminal()
                && self
                    .verbose
                    .log_level()
                    .is_some_and(|level| level >= Level::Warn)
                && let Some(trailers) = conn.response_trailers()
            {
                println!("\n{}", "Response Trailers".bold().underline());
                print_headers(trailers);
            }
        });
    }
}

fn print_headers(headers: &Headers) {
    for (name, values) in headers {
        for value in values {
            println!("{}: {}", name.as_ref().italic().bright_blue(), value);
        }
    }
}

pub(crate) fn parse_method_case_insensitive(src: &str) -> Result<Method, String> {
    src.to_uppercase()
        .parse()
        .map_err(|_| format!("unrecognized method {}", src))
}

pub(crate) fn parse_header(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    Ok((String::from(&s[..pos]), String::from(&s[pos + 1..])))
}

fn bytes(bytes: u64) -> String {
    size::Size::from_bytes(bytes)
        .format()
        .with_base(size::Base::Base10)
        .to_string()
}

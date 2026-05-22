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
    Body, Conn, Error, Headers, KnownHeaderName, Method, Status, Url, Version,
};
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

    /// tls implementation
    ///
    /// requests to https:// urls with `none` will fail
    #[arg(short, long, verbatim_doc_comment, value_enum, default_value_t)]
    tls: Tls,

    /// http version
    #[arg(long, verbatim_doc_comment, value_enum, default_value_t)]
    http_version: HttpVersion,

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
    #[arg(long, default_value_t = 10, conflicts_with = "no_follow_redirects", help_heading = "Redirects")]
    max_redirects: u32,

    /// follow redirects from https to http (blocked by default)
    #[arg(long, conflicts_with = "no_follow_redirects", help_heading = "Redirects")]
    allow_downgrade: bool,

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

impl ClientCli {
    async fn build(&self) -> Conn {
        // `Option<T>` is itself a `ClientHandler`, so a `None` here drops the
        // follow-redirects step entirely rather than capping it at zero.
        let client = crate::tls::build_client(self.tls, self.insecure).with_handler((
            std::io::stdout().is_terminal().then(ClientLogger::new),
            (!self.no_follow_redirects).then(|| {
                FollowRedirects::new()
                    .with_max_redirects(self.max_redirects)
                    .with_allow_downgrade(self.allow_downgrade)
            }),
        ));
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
            conn.set_request_body(Body::new_streaming(Unblock::new(std::io::stdin()), None));
        }

        conn
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
        } else if !std::io::stdin().is_terminal() {
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
        futures_lite::future::block_on(async move {
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

use blocking::Unblock;
use clap::Parser;
use clap_verbosity_flag::Verbosity;
use colored::*;
use log::Level;
use std::{
    io::{ErrorKind, IsTerminal},
    path::PathBuf,
    str::FromStr,
};
use trillium::{Body, Headers, Method, Status};
use trillium_client::{Client, Conn, Error};
use trillium_native_tls::NativeTlsConfig;
use trillium_rustls::RustlsConfig;
use trillium_smol::ClientConfig;
use url::{self, Url};
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

    /// tls implementation. options: rustls, native-tls, none
    ///
    /// requests to https:// urls with `none` will fail
    #[arg(short, long, default_value = "rustls", verbatim_doc_comment)]
    tls: TlsType,

    /// set the log level. add more flags for more verbosity
    ///
    /// example:
    /// trillium client get https://www.google.com -vvv # `trace` verbosity level
    #[command(flatten)]
    verbose: Verbosity,
}

impl ClientCli {
    async fn build(&self) -> Conn {
        let client = Client::from(self.tls);
        log::trace!("{}", self.url.as_str());
        let mut conn = client.build_conn(self.method, self.url.clone());

        conn.request_headers().extend(self.headers.clone());

        if let Some(path) = &self.file {
            let metadata = async_fs::metadata(path)
                .await
                .unwrap_or_else(|e| panic!("could not read file {:?} ({})", path, e));

            let file = async_fs::File::open(path)
                .await
                .unwrap_or_else(|e| panic!("could not read file {:?} ({})", path, e));

            conn.set_request_body(Body::new_streaming(file, Some(metadata.len())))
        } else if let Some(body) = &self.body {
            conn.set_request_body(body.clone())
        } else if atty::isnt(atty::Stream::Stdin) {
            conn.set_request_body(Body::new_streaming(Unblock::new(std::io::stdin()), None))
        }

        conn
    }

    pub fn run(self) {
        futures_lite::future::block_on(async move {
            env_logger::Builder::new()
                .filter_level(self.verbose.log_level_filter())
                .init();

            let mut conn = self.build().await;

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
                println!(
                    "{}: {}",
                    "Status".italic(),
                    if status.is_client_error() {
                        status.to_string().yellow()
                    } else if status.is_server_error() {
                        status.to_string().bright_red()
                    } else {
                        status.to_string().bright_green()
                    }
                );

                match self.verbose.log_level() {
                    Some(level) if level >= Level::Warn => {
                        println!("\n{}", "Request Headers".bold().underline());
                        print_headers(conn.request_headers());
                        println!("\n{}", "Response Headers".bold().underline());
                        print_headers(conn.response_headers());

                        println!("\n{}", "Body".bold().underline());
                    }
                    _ => {}
                }

                futures_lite::io::copy(
                    &mut conn.response_body(),
                    &mut Unblock::new(std::io::stdout()),
                )
                .await
                .unwrap();
            } else {
                futures_lite::io::copy(
                    &mut conn.response_body(),
                    &mut Unblock::new(std::io::stdout()),
                )
                .await
                .unwrap();
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

#[derive(clap::ValueEnum, Debug, Eq, PartialEq, Clone, Copy)]
pub enum TlsType {
    None,
    Rustls,
    Native,
}

impl From<TlsType> for Client {
    fn from(value: TlsType) -> Self {
        match value {
            TlsType::None => Client::new(ClientConfig::default()),
            TlsType::Rustls => Client::new(RustlsConfig::<ClientConfig>::default()),
            TlsType::Native => Client::new(NativeTlsConfig::<ClientConfig>::default()),
        }
    }
}

fn parse_method_case_insensitive(src: &str) -> Result<Method, String> {
    src.to_uppercase()
        .parse()
        .map_err(|_| format!("unrecognized method {}", src))
}

pub fn parse_url(src: &str) -> Result<Url, url::ParseError> {
    if src.starts_with("http") {
        src.parse()
    } else {
        format!("http://{}", src).parse()
    }
}

impl FromStr for TlsType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match &*s.to_ascii_lowercase() {
            "none" => Ok(Self::None),
            "rustls" => Ok(Self::Rustls),
            "native" | "native-tls" => Ok(Self::Native),
            _ => Err(format!("unrecognized tls {}", s)),
        }
    }
}

fn parse_header(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    Ok((String::from(&s[..pos]), String::from(&s[pos + 1..])))
}

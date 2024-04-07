use crate::client_tls::{parse_url, ClientTls};
use blocking::Unblock;
use clap::Parser;
use clap_verbosity_flag::Verbosity;
use colored::*;
use log::Level;
use std::{
    io::{ErrorKind, IsTerminal},
    path::PathBuf,
};
use trillium_client::{Body, Client, Conn, Error, Headers, Method, Status, Url};

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
    tls: ClientTls,

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

        conn.request_headers_mut().extend(self.headers.clone());

        if let Some(path) = &self.file {
            let file = async_fs::File::open(path)
                .await
                .unwrap_or_else(|e| panic!("could not read file {:?} ({})", path, e));

            let metadata = file.metadata().await.unwrap();

            conn.set_request_body(Body::new_streaming(file, Some(metadata.len())))
        } else if let Some(body) = &self.body {
            conn.set_request_body(body.clone())
        } else if !std::io::stdin().is_terminal() {
            conn.set_request_body(Body::new_streaming(Unblock::new(std::io::stdin()), None))
        }

        conn
    }

    async fn output_body(&self, conn: &mut Conn) {
        if let Some(file) = &self.output_file {
            let filename = file
                .clone()
                .unwrap_or_else(|| conn.url().path_segments().unwrap().last().unwrap().into());
            if filename.to_str().map_or(true, |f| f.is_empty()) {
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
                .get_str(trillium_client::KnownHeaderName::ContentType)
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

            self.output_body(&mut conn).await
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

fn parse_method_case_insensitive(src: &str) -> Result<Method, String> {
    src.to_uppercase()
        .parse()
        .map_err(|_| format!("unrecognized method {}", src))
}

fn parse_header(s: &str) -> Result<(String, String), String> {
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

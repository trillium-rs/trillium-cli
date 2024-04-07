use broadcaster::BroadcastChannel;
use clap::Parser;
use log::LevelFilter;
use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use signal_hook::{
    consts::signal::{SIGHUP, SIGUSR1},
    iterator::Signals,
};
use std::{
    env,
    io::{self, Write},
    path::PathBuf,
    process::Command,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

#[derive(Parser, Debug)]
pub struct DevServer {
    // /// Local host or ip to listen on
    // #[arg(short = "o", long, env, default_value = "localhost")]
    // host: String,

    // /// Local port to listen on
    // #[arg(short, long, env, default_value = "8080")]
    // port: u16,
    #[arg(short, long, env, default_value = "src")]
    watch: Option<Vec<PathBuf>>,

    #[arg(short, long, env)]
    bin: Option<PathBuf>,

    #[arg(short, long)]
    cwd: Option<PathBuf>,

    #[arg(short, long)]
    release: bool,

    #[arg(short, long)]
    example: Option<String>,

    #[arg(short, long, default_value = "SIGTERM")]
    signal: Signal,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum Event {
    BinaryChanged,
    Rebuild,
    Restarted,
    BuildSuccess,
    CompileError { error: String },
}

impl DevServer {
    fn determine_bin(&self) -> PathBuf {
        if let Some(ref bin) = self.bin {
            bin.canonicalize().unwrap()
        } else {
            let metadata = Command::new("cargo")
                .current_dir(self.cwd.clone().unwrap())
                .args(["metadata", "--format-version", "1"])
                .output()
                .unwrap();

            let value: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
            let target_dir =
                PathBuf::from(value.get("target_directory").unwrap().as_str().unwrap());

            let root = value
                .get("resolve")
                .unwrap()
                .get("root")
                .unwrap()
                .as_str()
                .unwrap()
                .split(' ')
                .next()
                .unwrap();

            let target_dir = target_dir.join(if self.release { "release" } else { "debug" });
            let target_dir = if let Some(example) = &self.example {
                target_dir.join("examples").join(example)
            } else {
                target_dir.join(root)
            };

            target_dir.canonicalize().unwrap()
        }
    }

    pub fn run(mut self) {
        env_logger::Builder::new()
            .filter_level(LevelFilter::Debug)
            .init();

        let cwd = self
            .cwd
            .get_or_insert_with(|| env::current_dir().unwrap())
            .clone();

        let bin = self.determine_bin();

        let mut run = Command::new(&bin);
        run.current_dir(&cwd);

        let mut build = Command::new("cargo");
        let mut args = vec!["build", "--color=always"];
        if self.release {
            args.push("--release");
        }

        let signal = self.signal;

        if let Some(example) = &self.example {
            args.push("--example");
            args.push(example);
            self.watch
                .get_or_insert_with(Vec::new)
                .push(cwd.join("examples"));
        }

        build.args(&args[..]);
        build.current_dir(&cwd);

        let mut child = run.spawn().unwrap();
        let child_id = Arc::new(Mutex::new(child.id()));

        let (tx, rx) = mpsc::channel();
        let broadcaster = BroadcastChannel::new();

        {
            let tx = tx.clone();
            thread::spawn(move || {
                let mut signals = Signals::new([SIGHUP, SIGUSR1]).unwrap();

                loop {
                    for signal in signals.pending() {
                        if let SIGHUP = signal as libc::c_int {
                            tx.send(Event::BinaryChanged).unwrap();
                        }
                    }
                }
            });
        }

        thread::spawn(move || {
            let (t, r) = mpsc::channel();
            let mut watcher = RecommendedWatcher::new(t, Default::default()).unwrap();

            if let Some(watches) = self.watch {
                for watch in watches {
                    let watch = if watch.is_relative() {
                        cwd.join(watch)
                    } else {
                        watch
                    };

                    let watch = watch.canonicalize().unwrap();
                    log::info!("watching {:?}", &watch);
                    watcher.watch(&watch, RecursiveMode::Recursive).unwrap();
                }
            }

            log::info!("watching {:?}", &bin);
            watcher.watch(&bin, RecursiveMode::NonRecursive).unwrap();

            while let Ok(Ok(m)) = r.recv() {
                for path in m.paths {
                    if let Ok(path) = path.canonicalize() {
                        if path == bin {
                            tx.send(Event::BinaryChanged).unwrap();
                        } else {
                            tx.send(Event::Rebuild).unwrap();
                        }
                    }
                }
            }
        });

        {
            let child_id = child_id.clone();
            let broadcaster = broadcaster.clone();
            thread::spawn(move || loop {
                child.wait().unwrap();
                log::info!("shut down, restarting");
                child = run.spawn().unwrap();
                *child_id.lock().unwrap() = child.id();
                thread::sleep(Duration::from_millis(500));
                async_io::block_on(broadcaster.send(&Event::Restarted)).ok();
            });
        }
        {
            let broadcaster = broadcaster.clone();
            thread::spawn(move || loop {
                let event = rx.recv().unwrap();
                async_io::block_on(broadcaster.send(&event)).unwrap();
                match event {
                    Event::BinaryChanged => {
                        log::info!("attempting to send {}", &signal);
                        signal::kill(
                            Pid::from_raw((*child_id.lock().unwrap()).try_into().unwrap()),
                            signal,
                        )
                        .unwrap();
                    }
                    Event::Rebuild => {
                        log::info!("building...");
                        let output = build.output();
                        match output {
                            Ok(ok) => {
                                if ok.status.success() {
                                    log::debug!("{}", String::from_utf8_lossy(&ok.stdout[..]));
                                    async_io::block_on(broadcaster.send(&Event::BuildSuccess)).ok();
                                } else {
                                    io::stderr().write_all(&ok.stderr).unwrap();
                                    async_io::block_on(
                                        broadcaster.send(&Event::CompileError {
                                            error: ansi_to_html::convert(&String::from_utf8_lossy(
                                                &ok.stderr,
                                            ))
                                            .unwrap(),
                                        }),
                                    )
                                    .ok();
                                }
                            }
                            Err(e) => {
                                eprintln!("{:?}", e);
                            }
                        }
                    }
                    _ => {}
                }
            });
        }

        proxy_app::run(format!("http://{}:{}", "localhost", "8080"), broadcaster);
    }
}

mod proxy_app {
    use super::Event;
    use broadcaster::BroadcastChannel;
    use futures_lite::StreamExt;
    use trillium::{Conn, KnownHeaderName, State};
    use trillium_client::Client;
    use trillium_html_rewriter::{
        html::{element, html_content::ContentType, Settings},
        HtmlRewriter,
    };
    use trillium_proxy::Proxy;
    use trillium_router::Router;
    use trillium_smol::ClientConfig;
    use trillium_websockets::{WebSocket, WebSocketConn};

    pub fn run(proxy: String, rx: BroadcastChannel<Event>) {
        static PORT: u16 = 8082;
        let client = Client::new(ClientConfig {
            nodelay: Some(true),
            ..Default::default()
        })
        .with_default_pool();

        trillium_smol::config()
            .without_signals()
            .with_port(PORT)
            .run((
                Router::new()
                    .get("/_dev_server.js", |conn: Conn| async move {
                        conn.with_response_header(
                            KnownHeaderName::ContentType,
                            "application/javascript",
                        )
                        .ok(include_str!("./dev_server.js"))
                    })
                    .get(
                        "/_dev_server.ws",
                        (
                            State::new(rx),
                            WebSocket::new(|mut conn: WebSocketConn| async move {
                                let mut rx = conn.take_state::<BroadcastChannel<Event>>().unwrap();
                                while let Some(message) = rx.next().await {
                                    if let Err(e) = conn.send_json(&message).await {
                                        log::error!("{:?}", e);
                                        return;
                                    }
                                }
                            }),
                        ),
                    ),
                Proxy::new(client, &*proxy),
                HtmlRewriter::new(|| Settings {
                    element_content_handlers: vec![element!("body", |el| {
                        el.append(
                            r#"<script src="/_dev_server.js"></script>"#,
                            ContentType::Html,
                        );
                        Ok(())
                    })],

                    ..Settings::default()
                }),
            ));
    }
}

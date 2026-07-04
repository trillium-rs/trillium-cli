use clap::Parser;
use clap_verbosity_flag::{InfoLevel, Verbosity};
use colored::Colorize;
use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use signal_hook::{consts::signal::SIGHUP, iterator::Signals};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    env,
    fmt::Display,
    io::{self, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};
use trillium::Conn;
use trillium_logger::LogFormatter;
use trillium_proxy::upstream::UpstreamSelector;
use url::Url;

/// The upstream the proxy forwards to, with a port that's updated on each
/// (re)spawn so we can rotate ports between rebuilds without dropping the old
/// child's in-flight connections. A port of `0` is the "no app yet" sentinel:
/// the app hasn't come up for the first time, so there's nothing to proxy to
/// and the readiness gate serves a "starting up" page instead.
#[derive(Debug, Clone)]
struct DynamicUpstream {
    host: String,
    port: Arc<AtomicU16>,
}

impl DynamicUpstream {
    fn new(host: String) -> Self {
        Self {
            host,
            port: Arc::new(AtomicU16::new(0)),
        }
    }

    fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    /// Whether the app has come up at least once and can be proxied to. Until
    /// then (port 0) there's no upstream and we serve the "starting up" page.
    fn is_ready(&self) -> bool {
        self.port.load(Ordering::Relaxed) != 0
    }

    fn upstream_base(&self) -> Option<Url> {
        let port = self.port.load(Ordering::Relaxed);
        // `None` here makes the proxy 502; the readiness gate ahead of it should
        // have already served the "starting up" page, so this is just a guard.
        if port == 0 {
            return None;
        }
        format!("http://{}:{port}", self.host).parse().ok()
    }
}

impl UpstreamSelector for DynamicUpstream {
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        self.upstream_base()?.determine_upstream(conn)
    }
}

impl LogFormatter for DynamicUpstream {
    type Output = u16;

    fn format(&self, _conn: &Conn, _color: bool) -> Self::Output {
        self.port.load(Ordering::Relaxed)
    }
}

#[derive(Parser, Debug)]
pub struct DevServer {
    /// Local host or ip for the dev server to listen on
    ///
    /// This is the address you point your browser at. The dev server adopts
    /// your `HOST`/`PORT`, then runs your app on a private port behind it.
    #[arg(short = 'o', long, env, default_value = "localhost")]
    host: String,

    /// Local port for the dev server to listen on
    #[arg(short, long, env, default_value = "8080")]
    port: u16,

    /// Extra directories to watch for changes (repeated; added to the default)
    ///
    /// By default the dev server watches the `src` of the crate it builds plus
    /// every workspace-local crate that one depends on, so editing a path
    /// dependency rebuilds too. Anything passed here is watched in addition.
    #[arg(short, long)]
    watch: Vec<PathBuf>,

    /// Paths to ignore, even when nested inside a watched directory (repeated)
    ///
    /// The watcher is recursive, so a build-output directory living under a
    /// watched tree — e.g. a frontend `dist/` that gets embedded in the binary —
    /// will otherwise retrigger the very build that produced it, an endless
    /// loop. List such paths here to break it. Matches a path and everything
    /// under it; relative paths are resolved against `--cwd`.
    #[arg(short, long, value_name = "PATH")]
    ignore: Vec<PathBuf>,

    /// Working directory to build and run in (defaults to the current dir)
    #[arg(short, long)]
    cwd: Option<PathBuf>,

    /// Build and run in release mode (also disables the dev build speedups)
    #[arg(short, long)]
    release: bool,

    /// Build and run the named example instead of the default binary
    #[arg(short, long)]
    example: Option<String>,

    /// Proxy to this fixed upstream port instead of auto-allocating one
    ///
    /// Use this only when your app hardcodes its listen port. Normally the dev
    /// server picks a free port and hands it to your app via the `PORT` env var.
    #[arg(long, env)]
    app_port: Option<u16>,

    /// Upstream host the app binds (passed to the app as `HOST`)
    #[arg(long, default_value = "localhost")]
    app_host: String,

    /// Disable dev build speedups (reduced debuginfo + fast-linker detection)
    ///
    /// The speedups change the build fingerprint, so toggling them forces one
    /// full rebuild.
    #[arg(long)]
    no_fast: bool,

    /// Editor used to open files from the error overlay (defaults to $EDITOR)
    ///
    /// May include arguments, e.g. `--editor "code --wait"`. The dev server
    /// appends the file and line/column in the right syntax for known editors
    /// (emacs/emacsclient, vim, code, subl, zed, JetBrains).
    #[arg(long, env = "EDITOR")]
    editor: Option<String>,

    /// Signal used to ask the app to shut down before a restart
    #[arg(short, long, default_value = "SIGTERM")]
    signal: Signal,

    #[command(flatten)]
    verbose: Verbosity<InfoLevel>,

    /// Arguments for `cargo build`, to select what gets built
    ///
    /// A single shell-quoted string, split like a shell would (repeatable; each
    /// occurrence is appended). Cargo resolves the binary, so the dev server
    /// runs whatever it produces.
    ///
    /// Examples:
    ///    `--build-args "-p my-crate"`
    ///    `--build-args "--bin worker --features dev"`
    #[arg(long, verbatim_doc_comment, allow_hyphen_values = true)]
    build_args: Vec<String>,

    /// Arguments passed to your app every time it starts
    ///
    /// A single shell-quoted string, split like a shell would (repeatable). Use
    /// this when your binary needs a subcommand or runtime flags before it does
    /// its thing — e.g. an app whose first argument selects `serve`.
    ///
    /// Examples:
    ///    `--run-args serve`
    ///    `--run-args "serve --verbose"`
    #[arg(long, verbatim_doc_comment, allow_hyphen_values = true)]
    run_args: Vec<String>,
}

/// Events broadcast to connected browsers over the dev-server websocket.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum Event {
    /// A rebuild has started.
    Rebuild,
    /// The app was rebuilt and restarted; the page should reload.
    Restarted,
    /// A rebuild finished successfully.
    BuildSuccess,
    /// A rebuild failed; render these diagnostics in the browser overlay.
    CompileError { diagnostics: Vec<Diagnostic> },
}

/// A source location the dev server is willing to open in an editor.
///
/// Built from compiler output and held server-side; the browser only ever
/// references one by `id`, never by path — so a hostile page can't ask us to
/// open an arbitrary file (which, for editors like emacs/vim, is a code-exec
/// vector via file-local variables / modelines).
#[derive(Debug, Clone)]
struct EditorTarget {
    path: PathBuf,
    line: u32,
    column: u32,
}

/// A single compiler error, shaped for the browser overlay.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// `error` / `warning`.
    level: String,
    /// The primary message line.
    message: String,
    /// rustc's full rendered output for this diagnostic, as HTML.
    rendered: String,
    /// Source location of the primary span, for display (relative path).
    file: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    /// Index into the server's open-target table; `Some` when this diagnostic
    /// has a location the editor can jump to. Assigned in [`Supervisor::build`].
    id: Option<usize>,
    /// The actual location to open, kept server-side and never serialized.
    #[serde(skip)]
    target: Option<EditorTarget>,
}

/// The relevant subset of a rustc JSON diagnostic (the `message` of a
/// `compiler-message` record).
#[derive(Debug, Deserialize)]
struct RustcMessage {
    message: String,
    level: String,
    #[serde(default)]
    spans: Vec<RustcSpan>,
    rendered: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RustcSpan {
    file_name: String,
    line_start: u32,
    column_start: u32,
    is_primary: bool,
}

impl Diagnostic {
    fn from_rustc(msg: RustcMessage, cwd: &Path) -> Self {
        let primary = msg
            .spans
            .iter()
            .find(|s| s.is_primary)
            .or(msg.spans.first());
        let (file, line, column, target) = match primary {
            Some(span) => (
                Some(span.file_name.clone()),
                Some(span.line_start),
                Some(span.column_start),
                Some(EditorTarget {
                    path: cwd.join(&span.file_name),
                    line: span.line_start,
                    column: span.column_start,
                }),
            ),
            None => (None, None, None, None),
        };
        let rendered = msg.rendered.unwrap_or_default();
        let rendered = ansi_to_html::convert(&rendered).unwrap_or(rendered);
        Diagnostic {
            level: msg.level,
            message: msg.message,
            rendered,
            file,
            line,
            column,
            id: None,
            target,
        }
    }
}

impl DevServer {
    pub fn run(self) {
        env_logger::Builder::new()
            .filter_level(self.verbose.log_level_filter())
            .format(|buf, record| {
                writeln!(
                    buf,
                    "[{}] {}",
                    record.module_path().unwrap_or_default().dimmed(),
                    record.args()
                )
            })
            .init();

        let cwd = self
            .cwd
            .clone()
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|e| die(e)));
        let cwd = cwd
            .canonicalize()
            .unwrap_or_else(|e| die(format!("{}: {e}", cwd.display())));

        // `--build-args`/`--run-args` each take a shell-quoted string; split them
        // the way a shell would so quoting works as written. Build args select
        // what cargo compiles; run args are handed to the app on every start.
        let build_args = shell_split("--build-args", &self.build_args);
        let app_args = shell_split("--run-args", &self.run_args);

        // The dev server adopts the user's public HOST/PORT; the app is moved
        // onto a private port, chosen per-spawn (a fresh one each rebuild unless
        // --app-port pins it), so nothing is allocated up front here.

        // Build command (reused for every rebuild). We ask cargo for JSON so it
        // tells us exactly which binary it produced and gives us structured
        // diagnostics, rather than reconstructing a target path ourselves.
        let mut build = Command::new("cargo");
        build
            .current_dir(&cwd)
            .args(["build", "--message-format=json-diagnostic-rendered-ansi"])
            // Let cargo's human-readable side — `Compiling …`, the progress bar,
            // the `Finished`/error summary — stream straight to our terminal, so a
            // slow rebuild visibly *does* something instead of hanging silently.
            // stdout stays piped: it's the JSON we parse for diagnostics and the
            // produced binary, not something to show a human.
            .stderr(Stdio::inherit());
        if self.release {
            build.arg("--release");
        }
        if let Some(example) = &self.example {
            build.args(["--example", example]);
        }
        build.args(&build_args);
        if !self.no_fast && !self.release {
            apply_fast_build(&mut build);
        }

        // Watch scope is small by default: the src of the crate cargo will
        // build (resolved from `-p`/the workspace), plus anything the user adds.
        let watches = resolve_watch_dirs(&cwd, &build_args, self.example.is_some(), &self.watch);
        let ignores = resolve_ignore_dirs(&cwd, &self.ignore);

        print_banner(&self, &watches, &ignores);

        let (tx, rx) = mpsc::channel::<()>();
        let (mut broadcast_tx, broadcast_rx) = async_broadcast::broadcast::<Event>(16);
        // Never block the supervisor on a slow/absent browser: drop the oldest
        // event when the buffer is full, and don't wait for a receiver to exist.
        broadcast_tx.set_overflow(true);
        broadcast_tx.set_await_active(false);
        // Hold one receiver for the whole run so the channel stays open even
        // when no browser is connected (sends would otherwise error).
        let _keepalive_rx = broadcast_rx;

        // External SIGHUP triggers a rebuild (e.g. from another tool).
        {
            let tx = tx.clone();
            thread::spawn(move || {
                let mut signals = Signals::new([SIGHUP]).unwrap_or_else(|e| die(e));
                for _ in signals.forever() {
                    if tx.send(()).is_err() {
                        break;
                    }
                }
            });
        }

        // Filesystem watcher.
        {
            let tx = tx.clone();
            let cwd = cwd.clone();
            thread::spawn(move || watch_loop(watches, ignores, cwd, tx));
        }

        // Shared with the websocket handler: locations the error overlay may
        // open, and the current build status to replay to new connections.
        let open_targets = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(Mutex::new(None));

        // Create a shared upstream selector that can be updated on each rebuild.
        // It starts "not ready" (port 0) until the first spawn comes up.
        let upstream = DynamicUpstream::new(self.app_host.clone());

        // Supervisor: owns the child process, builds, and restarts.
        {
            let broadcaster = broadcast_tx.clone();
            let app_host = self.app_host.clone();
            let signal = self.signal;
            let open_targets = open_targets.clone();
            let status = status.clone();
            let pinned_port = self.app_port;
            let upstream = upstream.clone();
            thread::spawn(move || {
                Supervisor {
                    rx,
                    broadcaster,
                    build,
                    cwd,
                    signal,
                    app_host,
                    pinned_port,
                    upstream,
                    app_args,
                    exe: None,
                    child: None,
                    open_targets,
                    status,
                }
                .run()
            });
        }

        let editor = self.editor.clone().or_else(|| env::var("VISUAL").ok());
        proxy_app::run(
            self.host.clone(),
            self.port,
            upstream,
            broadcast_tx,
            editor,
            open_targets,
            status,
        );
    }
}

/// Owns the child process and drives build/restart in response to triggers.
struct Supervisor {
    rx: mpsc::Receiver<()>,
    broadcaster: async_broadcast::Sender<Event>,
    build: Command,
    cwd: PathBuf,
    signal: Signal,
    app_host: String,
    /// Port for the app; fixed if --app-port was given, otherwise we rotate for each spawn.
    pinned_port: Option<u16>,
    /// Shared upstream selector for the proxy to dynamically determine the app port.
    upstream: DynamicUpstream,
    /// Args forwarded to the app on every start (from `--run-args`).
    app_args: Vec<String>,
    /// The executable cargo produced for the most recent successful build.
    exe: Option<PathBuf>,
    child: Option<Child>,
    /// Locations the editor may open, shared with the websocket handler.
    open_targets: Arc<Mutex<Vec<EditorTarget>>>,
    /// The latest sticky status, replayed to browsers as they connect.
    status: Arc<Mutex<Option<Event>>>,
}

impl Supervisor {
    fn broadcast(&self, event: Event) {
        // Remember the current build state so a browser connecting (or
        // reconnecting) later can be brought up to date immediately.
        // `Restarted` is a one-shot action, not a state — replaying it would
        // reload every newly-connected page forever — so it's never sticky.
        {
            let mut status = self.status.lock().unwrap();
            match &event {
                Event::Rebuild | Event::CompileError { .. } => *status = Some(event.clone()),
                Event::BuildSuccess => *status = None,
                Event::Restarted => {}
            }
        }
        let _ = async_io::block_on(self.broadcaster.broadcast_direct(event));
    }

    /// Move each diagnostic's location into the shared open-target table and
    /// stamp the diagnostic with its `id`, so the browser can request an open
    /// by index rather than by (forgeable) path.
    fn publish_targets(&self, diagnostics: &mut [Diagnostic]) {
        let mut targets = Vec::new();
        for diagnostic in diagnostics {
            if let Some(target) = diagnostic.target.take() {
                diagnostic.id = Some(targets.len());
                targets.push(target);
            }
        }
        *self.open_targets.lock().unwrap() = targets;
    }

    /// Run `cargo build`, parsing its JSON output for the produced executable
    /// and any diagnostics. Broadcasts success or rendered compile errors.
    fn build(&mut self) -> bool {
        log::info!("building…");
        let output = match self.build.output() {
            Ok(output) => output,
            Err(e) => {
                log::error!("failed to run `cargo build`: {e}");
                return false;
            }
        };

        // cargo emits one JSON object per line on stdout: `compiler-artifact`
        // carries the executable path, `compiler-message` carries diagnostics.
        let mut executables = Vec::new();
        let mut diagnostics = Vec::new();
        let mut terminal = String::new();
        for line in output.stdout.split(|&b| b == b'\n') {
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            match value.get("reason").and_then(|r| r.as_str()) {
                Some("compiler-artifact") => {
                    if let Some(exe) = value.get("executable").and_then(|e| e.as_str()) {
                        executables.push(PathBuf::from(exe));
                    }
                }
                Some("compiler-message") => {
                    let Some(message) = value.get("message").cloned() else {
                        continue;
                    };
                    let Ok(msg) = serde_json::from_value::<RustcMessage>(message) else {
                        continue;
                    };
                    // Everything goes to the terminal; only errors get an overlay card.
                    if let Some(rendered) = &msg.rendered {
                        terminal.push_str(rendered);
                    }
                    if msg.level == "error" {
                        diagnostics.push(Diagnostic::from_rustc(msg, &self.cwd));
                    }
                }
                _ => {}
            }
        }

        if !output.status.success() {
            // cargo's progress and top-level error summary already streamed live
            // to the terminal (stderr is inherited). The per-error diagnostics,
            // though, only arrived as JSON on stdout — echo their rendered form so
            // the terminal actually shows *what* failed, not just "could not
            // compile due to N errors".
            if !terminal.trim().is_empty() {
                io::stderr().write_all(terminal.as_bytes()).ok();
            }
            if diagnostics.is_empty() {
                // No structured diagnostics: cargo itself refused (e.g. an unknown
                // `-p`). That message already went to the terminal, but the browser
                // overlay has nothing to render, so send it there.
                diagnostics.push(Diagnostic {
                    level: "error".into(),
                    message: "build failed".into(),
                    rendered: if terminal.trim().is_empty() {
                        "build failed — see the dev-server terminal for details".into()
                    } else {
                        ansi_to_html::convert(&terminal).unwrap_or(terminal)
                    },
                    file: None,
                    line: None,
                    column: None,
                    id: None,
                    target: None,
                });
            }
            self.publish_targets(&mut diagnostics);
            self.broadcast(Event::CompileError { diagnostics });
            return false;
        }

        if executables.len() > 1 {
            log::warn!(
                "build produced {} binaries; running the last. Use `--build-args \"--bin \
                 <name>\"` to pick one.",
                executables.len()
            );
        }
        match executables.pop() {
            Some(exe) => {
                log::info!("{}", "build succeeded".green());
                self.exe = Some(exe);
                self.broadcast(Event::BuildSuccess);
                true
            }
            None => {
                log::error!(
                    "build succeeded but produced no runnable binary — is this a binary crate? \
                     try `--example <name>` or `--build-args \"--bin <name>\"`"
                );
                false
            }
        }
    }

    /// Start the app on its port (freshly allocated unless `--app-port` pinned
    /// it), wait for it to come up, and only then point the proxy at it — so any
    /// previous child keeps serving until the new one is ready. Returns true if
    /// the process was started (the caller may then retire the old child).
    fn spawn(&mut self) -> bool {
        let Some(exe) = self.exe.clone() else {
            return false;
        };

        // A fresh port per spawn (unless pinned) lets the outgoing child keep
        // serving on its old port while this one starts up on the new one.
        let port = self.pinned_port.unwrap_or_else(free_port);

        // A fresh Command each time: the executable path can change between
        // builds (e.g. debug ↔ release, or a different selected target).
        let mut command = Command::new(exe);
        command
            .args(&self.app_args)
            .current_dir(&self.cwd)
            .env("PORT", port.to_string())
            .env("HOST", &self.app_host)
            .env("TRILLIUM_CLI_DEV_SERVER", "1");

        match command.spawn() {
            Ok(child) => {
                self.child = Some(child);
                let listening = wait_until_listening(&self.app_host, port);
                // Flip the proxy over only now: while this child was starting
                // up, any previous child kept receiving traffic on its own port.
                self.upstream.set_port(port);
                if listening {
                    println!(
                        "{}",
                        format!("  app listening on {}:{}", self.app_host, port).green()
                    );
                } else {
                    println!(
                        "  {} app failed to bind to {}:{}",
                        "warning:".yellow(),
                        self.app_host,
                        port
                    );
                }
                true
            }
            Err(e) => {
                println!("  {} failed to start app: {}", "error:".red().bold(), e);
                false
            }
        }
    }

    /// Fully retire the current child, blocking until it's gone. Used on the
    /// fixed-port path, where the new child can't bind until this one lets go.
    fn stop(&mut self) {
        if let Some(child) = self.child.take() {
            reap_child(child, self.signal);
        }
    }

    /// Swap in the freshly-built app. With a rotating port the new child comes
    /// up on a fresh port and takes over the instant it's listening, while the
    /// previous child keeps draining its in-flight requests on the old port in
    /// the background — a true hot-deploy with no gap. With a pinned port the
    /// two can't coexist on the same port, so the old child is fully retired
    /// first (blocking for the length of its drain).
    fn hot_swap(&mut self) {
        if self.pinned_port.is_some() {
            self.stop();
            if self.spawn() {
                self.broadcast(Event::Restarted);
            }
            return;
        }

        // Keep the old child alive and serving until the new one is listening;
        // `spawn` flips the proxy over only once that happens.
        let old = self.child.take();
        if self.spawn() {
            if let Some(old) = old {
                let sig = self.signal;
                thread::spawn(move || reap_child(old, sig));
            }
            self.broadcast(Event::Restarted);
        } else {
            // The new build wouldn't even start; keep the old child serving.
            self.child = old;
        }
    }

    fn run(mut self) {
        if self.build() && self.spawn() {
            // Reload any browser sitting on the "starting up" page now that the
            // app has come up for the first time.
            self.broadcast(Event::Restarted);
        }

        loop {
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(()) => {
                    self.broadcast(Event::Rebuild);
                    if self.build() {
                        self.hot_swap();
                    }
                    // On failure the old child keeps running; the browser shows
                    // the CompileError overlay until the next good build.
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Reap an app that exited on its own (a crash).
                    if let Some(child) = self.child.as_mut()
                        && matches!(child.try_wait(), Ok(Some(_)))
                    {
                        log::warn!("app exited on its own; restarting");
                        self.child = None;
                        thread::sleep(Duration::from_millis(300)); // crash-loop backoff
                        if self.spawn() {
                            self.broadcast(Event::Restarted);
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// Retire a child process, escalating if it won't leave. First `sig` (SIGTERM
/// by default) asks trillium to drain in-flight connections and exit, and we
/// give it a generous window. If it's still alive we send `sig` again —
/// trillium treats a second SIGTERM as "exit hard, now" — and wait briefly.
/// Only if even that is ignored do we STONITH with SIGKILL, on the assumption
/// the process is asleep at the wheel and will never come back on its own.
///
/// Blocks for as long as the drain takes, so callers that need the new child
/// serving meanwhile should run this on a background thread.
fn reap_child(mut child: Child, sig: Signal) {
    let pid = Pid::from_raw(child.id() as i32);
    let graceful = Duration::from_secs(12);

    // 1. Graceful: let trillium drain in-flight connections and shut down.
    let _ = signal::kill(pid, sig);
    if wait_for_exit(&mut child, graceful) {
        log::info!("app exited gracefully after {sig}");
        return;
    }

    // 2. Impatient: a second signal tells trillium to exit hard.
    log::warn!(
        "app still running {}s after {sig}; sending it again for a hard exit",
        graceful.as_secs()
    );
    let _ = signal::kill(pid, sig);
    if wait_for_exit(&mut child, Duration::from_millis(100)) {
        log::info!("app exited after a second {sig}");
        return;
    }

    // 3. STONITH: the process is wedged; there is no node, only the process.
    log::warn!("app ignored two {sig} signals; sending SIGKILL");
    let _ = signal::kill(pid, Signal::SIGKILL);
    let _ = child.wait();
}

/// Poll `child` until it exits or `timeout` elapses. Returns true if it exited
/// (and reaps it); false on timeout so the caller can escalate.
fn wait_for_exit(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(e) => {
                log::warn!("failed to check child status: {e}");
                return false;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Block until something is listening on `host:port`, or give up after 10s.
/// Returns true if we connected successfully, false if we timed out.
fn wait_until_listening(host: &str, port: u16) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(addrs) = (host, port).to_socket_addrs() {
            for addr in addrs {
                if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                    log::info!("app is listening on {host}:{port}");
                    return true;
                }
            }
        }
        if Instant::now() >= deadline {
            log::warn!("app did not start listening on {host}:{port} within 10s");
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Watch the given directories and send `()` on any change, debounced.
///
/// `ignores` are absolute path prefixes filtered out of every event: the
/// watcher is recursive and `notify` can't exclude a nested subtree, so we drop
/// their events here instead. A burst whose paths are *all* ignored triggers no
/// rebuild — that's what keeps a build-output dir from rebuilding itself.
fn watch_loop(watches: Vec<PathBuf>, ignores: Vec<PathBuf>, cwd: PathBuf, tx: mpsc::Sender<()>) {
    let (events_tx, events_rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(events_tx, notify::Config::default())
        .unwrap_or_else(|e| die(format!("could not start file watcher: {e}")));

    for watch in watches {
        let path = if watch.is_relative() {
            cwd.join(&watch)
        } else {
            watch
        };
        match path.canonicalize() {
            Ok(path) => match watcher.watch(&path, RecursiveMode::Recursive) {
                Ok(()) => log::info!("watching {}", path.display()),
                Err(e) => log::warn!("could not watch {}: {e}", path.display()),
            },
            Err(_) => log::warn!("watch path does not exist: {}", path.display()),
        }
    }

    loop {
        // Block for the first event of a burst.
        let Ok(first) = events_rx.recv() else {
            return;
        };
        // Coalesce a burst of events (one save touches many files) into a
        // single rebuild by draining until things go quiet, collecting the
        // paths that changed so we can report what actually triggered it.
        // `relevant` tracks whether anything outside the ignore list moved: a
        // burst that's entirely ignored (e.g. a rebuilt `dist/`) must not
        // retrigger, while a genuine pathless event still should.
        let mut changed = BTreeSet::new();
        let mut relevant = record_changed(&mut changed, first, &cwd, &ignores);
        while let Ok(event) = events_rx.recv_timeout(Duration::from_millis(150)) {
            relevant |= record_changed(&mut changed, event, &cwd, &ignores);
        }

        if !relevant {
            // The whole burst landed inside an ignored path; stay quiet.
            continue;
        }

        if changed.is_empty() {
            log::info!("change detected; rebuilding");
        } else {
            let paths = changed
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            log::info!("change detected in {paths}; rebuilding");
        }

        if tx.send(()).is_err() {
            return;
        }
    }
}

/// Fold the paths from a notify event into `changed`, made relative to `cwd`
/// when possible so the reported trigger is short and readable. Paths under any
/// `ignores` prefix are dropped.
///
/// Returns whether the event is relevant to a rebuild: true if it carried at
/// least one non-ignored path, or no path at all (a pathless event can't be
/// attributed, so we treat it as relevant rather than silently swallow it);
/// false only when every path it named was ignored.
fn record_changed(
    changed: &mut BTreeSet<PathBuf>,
    event: notify::Result<notify::Event>,
    cwd: &Path,
    ignores: &[PathBuf],
) -> bool {
    let Ok(event) = event else { return false };
    if event.paths.is_empty() {
        return true;
    }
    let mut relevant = false;
    for path in event.paths {
        if ignores.iter().any(|ignore| path.starts_with(ignore)) {
            continue;
        }
        relevant = true;
        let path = path
            .strip_prefix(cwd)
            .map(Path::to_path_buf)
            .unwrap_or(path);
        changed.insert(path);
    }
    relevant
}

/// Apply build-time speedups for dev builds: trim debuginfo (the biggest link
/// cost) and use a fast linker when one is installed. Injected via env so the
/// user's `Cargo.toml` is untouched.
fn apply_fast_build(build: &mut Command) {
    build.env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only");
    let mut applied = String::from("reduced debuginfo");

    // Only inject a linker if the user hasn't set their own rustflags.
    if env::var_os("RUSTFLAGS").is_none()
        && env::var_os("CARGO_BUILD_RUSTFLAGS").is_none()
        && let Some((linker, flag)) = detect_fast_linker()
    {
        build.env("CARGO_BUILD_RUSTFLAGS", flag);
        applied.push_str(&format!(" + {linker} linker"));
    }

    log::info!("dev build speedups: {applied} (pass --no-fast to disable)");
}

fn detect_fast_linker() -> Option<(&'static str, &'static str)> {
    [
        ("mold", "mold", "-Clink-arg=-fuse-ld=mold"),
        ("lld", "ld.lld", "-Clink-arg=-fuse-ld=lld"),
    ]
    .into_iter()
    .find(|(_, probe, _)| {
        Command::new(probe)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
    .map(|(name, _, flag)| (name, flag))
}

/// Decide what to watch: the `src` of the crate cargo will build *and* its
/// workspace-local dependencies (so editing a path-dep you depend on rebuilds),
/// the `examples` dir when building one, plus any `--watch` directories.
fn resolve_watch_dirs(
    cwd: &Path,
    cargo_args: &[String],
    example: bool,
    explicit: &[PathBuf],
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let (seeds, closure) = workspace_crates(cwd, cargo_args);

    // The whole local dependency closure contributes its `src`; examples only
    // make sense for the crate we actually selected to run.
    for crate_dir in &closure {
        let src = crate_dir.join("src");
        if src.is_dir() {
            dirs.push(src);
        }
    }
    if example {
        for crate_dir in &seeds {
            let examples = crate_dir.join("examples");
            if examples.is_dir() {
                dirs.push(examples);
            }
        }
    }

    for watch in explicit {
        dirs.push(if watch.is_relative() {
            cwd.join(watch)
        } else {
            watch.clone()
        });
    }

    dirs.sort();
    dirs.dedup();
    if dirs.is_empty() {
        log::warn!("couldn't tell which crate to watch — pass `-p <crate>` or `--watch <dir>`");
    }
    dirs
}

/// Resolve `--ignore` paths to absolute prefixes for event filtering. Relative
/// paths are taken against `cwd`, and each is canonicalized so it matches the
/// real paths `notify` reports (the watcher resolves symlinks); a path that
/// doesn't exist yet falls back to its joined form so it still matches once it
/// appears.
fn resolve_ignore_dirs(cwd: &Path, ignores: &[PathBuf]) -> Vec<PathBuf> {
    ignores
        .iter()
        .map(|ignore| {
            let path = if ignore.is_relative() {
                cwd.join(ignore)
            } else {
                ignore.clone()
            };
            path.canonicalize().unwrap_or(path)
        })
        .collect()
}

/// Returns `(seed crate dirs, dependency-closure crate dirs)` for the build.
///
/// Seeds are the crates cargo will build (from `-p`, or the single member, or
/// the member containing the cwd). The closure additionally includes every
/// workspace-local crate reachable from the seeds through the resolved
/// dependency graph — registry dependencies are excluded.
fn workspace_crates(cwd: &Path, cargo_args: &[String]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let Some(meta) = cargo_metadata(cwd) else {
        return (Vec::new(), Vec::new());
    };

    // id -> crate directory, for every package in the graph.
    let mut dir_of: HashMap<&str, PathBuf> = HashMap::new();
    // id -> package name, for workspace members (used to resolve `-p`).
    let mut name_of: HashMap<&str, &str> = HashMap::new();
    if let Some(packages) = meta.get("packages").and_then(|p| p.as_array()) {
        for pkg in packages {
            if let (Some(id), Some(name), Some(manifest)) = (
                pkg.get("id").and_then(|v| v.as_str()),
                pkg.get("name").and_then(|v| v.as_str()),
                pkg.get("manifest_path").and_then(|v| v.as_str()),
            ) && let Some(dir) = Path::new(manifest).parent()
            {
                dir_of.insert(id, dir.to_path_buf());
                name_of.insert(id, name);
            }
        }
    }

    let members: HashSet<&str> = meta
        .get("workspace_members")
        .and_then(|m| m.as_array())
        .map(|ids| ids.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    // id -> resolved dependency ids.
    let mut deps_of: HashMap<&str, Vec<&str>> = HashMap::new();
    if let Some(nodes) = meta
        .get("resolve")
        .and_then(|r| r.get("nodes"))
        .and_then(|n| n.as_array())
    {
        for node in nodes {
            if let Some(id) = node.get("id").and_then(|v| v.as_str()) {
                let deps = node
                    .get("dependencies")
                    .and_then(|d| d.as_array())
                    .map(|d| d.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                deps_of.insert(id, deps);
            }
        }
    }

    let selected = packages_from_args(cargo_args);
    let seed_ids: Vec<&str> = if !selected.is_empty() {
        members
            .iter()
            .copied()
            .filter(|id| {
                name_of
                    .get(id)
                    .is_some_and(|n| selected.iter().any(|s| s == n))
            })
            .collect()
    } else if members.len() == 1 {
        members.iter().copied().collect()
    } else {
        // Virtual workspace with no `-p`: the member containing the cwd (the
        // most specific one, if nested).
        members
            .iter()
            .copied()
            .filter(|id| dir_of.get(id).is_some_and(|dir| cwd.starts_with(dir)))
            .max_by_key(|id| dir_of.get(id).map_or(0, |dir| dir.components().count()))
            .into_iter()
            .collect()
    };

    // Walk the dependency graph from the seeds, staying within the workspace.
    let mut closure_ids: HashSet<&str> = HashSet::new();
    let mut stack = seed_ids.clone();
    while let Some(id) = stack.pop() {
        if !members.contains(id) || !closure_ids.insert(id) {
            continue;
        }
        if let Some(deps) = deps_of.get(id) {
            stack.extend(deps.iter().filter(|d| members.contains(*d)));
        }
    }

    let dirs = |ids: &[&str]| {
        ids.iter()
            .filter_map(|id| dir_of.get(id).cloned())
            .collect()
    };
    let seeds = dirs(&seed_ids);
    let closure = dirs(&closure_ids.into_iter().collect::<Vec<_>>());
    (seeds, closure)
}

/// Run `cargo metadata` (with the resolved dependency graph) in `cwd`.
fn cargo_metadata(cwd: &Path) -> Option<serde_json::Value> {
    let output = Command::new("cargo")
        .current_dir(cwd)
        .args(["metadata", "--format-version", "1"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Split each shell-quoted `--build-args`/`--run-args` string into argv the way
/// a shell would, flattening repeated occurrences into one list. Dies with a
/// clear error (naming the flag) if a value has unbalanced quotes.
fn shell_split(flag: &str, values: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        match shlex::split(value) {
            Some(tokens) => out.extend(tokens),
            None => die(format!(
                "could not parse `{flag} {value:?}` — unbalanced quotes?"
            )),
        }
    }
    out
}

/// Extract package names from `-p`/`--package` arguments forwarded to cargo.
fn packages_from_args(args: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "-p" || arg == "--package" {
            if let Some(name) = args.next() {
                names.push(name.clone());
            }
        } else if let Some(name) = arg.strip_prefix("--package=") {
            names.push(name.to_string());
        } else if let Some(name) = arg.strip_prefix("-p").filter(|n| !n.is_empty()) {
            names.push(name.to_string());
        }
    }
    names
}

/// Bind to an ephemeral port to discover a free one, then release it.
fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| Ok(listener.local_addr()?.port()))
        .unwrap_or_else(|e| die(format!("could not allocate a port: {e}")))
}

fn print_banner(server: &DevServer, watches: &[PathBuf], ignores: &[PathBuf]) {
    let watches = watches
        .iter()
        .map(|w| w.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    println!();
    println!("  {} {}", "▲".green(), "trillium dev-server".bold());
    println!(
        "  {}   http://{}:{}",
        "proxy".dimmed(),
        server.host,
        server.port
    );
    // The app's port isn't announced here: it rotates per rebuild, so each
    // `app listening on …` line reports the real one as the service comes up.
    println!("  {}    {}", "watch".dimmed(), watches);
    if !ignores.is_empty() {
        let ignores = ignores
            .iter()
            .map(|i| i.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {}   {}", "ignore".dimmed(), ignores);
    }
    println!();
}

fn die(msg: impl Display) -> ! {
    eprintln!("{} {msg}", "error:".red().bold());
    std::process::exit(1);
}

/// A command sent from the browser over the websocket.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum BrowserCommand {
    /// Open a diagnostic's source location in the user's editor. `id` indexes
    /// the server's open-target table — the browser cannot name an arbitrary
    /// path.
    Open { id: usize },
}

/// Launch `editor` (which may include arguments) on `file`, jumping to
/// `line`/`column` using the argument syntax for known editors.
fn open_in_editor(editor: &str, file: &str, line: u32, column: u32) {
    let mut parts = editor.split_whitespace();
    let Some(program) = parts.next() else {
        log::warn!("$EDITOR is empty");
        return;
    };
    let base_args: Vec<&str> = parts.collect();
    let name = Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(program);

    let mut command = Command::new(program);
    command.args(&base_args);
    match name {
        "emacs" | "emacsclient" => {
            command.arg(format!("+{line}:{column}")).arg(file);
        }
        "vi" | "vim" | "nvim" | "gvim" | "mvim" => {
            command.arg(format!("+{line}")).arg(file);
        }
        "code" | "code-insiders" | "codium" | "vscodium" | "cursor" | "windsurf" => {
            command.arg("--goto").arg(format!("{file}:{line}:{column}"));
        }
        "subl" | "sublime_text" | "zed" => {
            command.arg(format!("{file}:{line}:{column}"));
        }
        "idea" | "pycharm" | "webstorm" | "rubymine" | "clion" | "goland" | "rustrover"
        | "phpstorm" => {
            command.arg("--line").arg(line.to_string()).arg(file);
        }
        _ => {
            command.arg(file);
        }
    }
    // Detach from the dev server's terminal (so a TUI editor can't grab it) and
    // don't wait for the editor to exit.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.spawn() {
        Ok(_) => log::info!("opening {file}:{line}:{column} in {program}"),
        Err(e) => log::warn!("could not open editor `{program}`: {e}"),
    }
}

mod proxy_app {
    use super::{BrowserCommand, EditorTarget, Event, open_in_editor};
    use async_broadcast::Sender;
    use futures_lite::{StreamExt, future};
    use std::sync::{Arc, Mutex};
    use trillium::{Conn, Handler, KnownHeaderName, State, Status};
    use trillium_client::Client;
    use trillium_compression::client::Compression;
    use trillium_html_rewriter::{
        HtmlRewriter,
        html::{Settings, element, html_content::ContentType},
    };
    use trillium_logger::{Logger, client::ClientLogger, log_format};
    use trillium_proxy::Proxy;
    use trillium_router::Router;
    use trillium_smol::ClientConfig;
    use trillium_websockets::{Message, WebSocket, WebSocketConn};

    /// Served in place of the proxy's BadGateway while the app isn't up yet. The
    /// live-reload script is injected into `<body>` by the HtmlRewriter (not
    /// baked in here), so the page reloads itself the moment the app is ready.
    const STARTING_UP_PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>starting up…</title>
<style>
  html, body { height: 100%; margin: 0; }
  body {
    display: flex; align-items: center; justify-content: center;
    background: #1e1e2e; color: #cdd6f4;
    font: 15px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .box { text-align: center; padding: 32px; }
  .spin {
    width: 28px; height: 28px; margin: 0 auto 20px;
    border: 3px solid #45475a; border-top-color: #a6e3a1;
    border-radius: 50%; animation: spin .8s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }
  h1 { font-size: 16px; font-weight: 600; margin: 0 0 8px; color: #a6e3a1; }
  p { margin: 0; color: #6c7086; font-size: 13px; }
</style>
</head>
<body>
  <div class="box">
    <div class="spin"></div>
    <h1>starting your trillium app…</h1>
    <p>the first build can take a moment — this page reloads automatically.</p>
  </div>
</body>
</html>"#;

    /// Serves the "starting up" page whenever there's no live app to reach:
    /// either the first build hasn't come up yet (upstream port still 0, so the
    /// proxy passes the conn through untouched — status `None`), or the app is
    /// momentarily unreachable during a crash/restart (proxy sets `BadGateway`).
    /// Both get an honest `503 Service Unavailable` with a readable body; a real
    /// status from the running app (a 500, a 404, anything) passes through.
    ///
    /// This runs in `before_send` rather than `run` because the proxy halts the
    /// conn on the BadGateway path, which skips the rest of the `run` chain;
    /// `before_send` fires on every handler regardless of halt.
    struct StartingUpPage {
        upstream: super::DynamicUpstream,
    }

    impl Handler for StartingUpPage {
        async fn run(&self, conn: Conn) -> Conn {
            // A reachable app that answered for itself: leave it alone.
            if !matches!(conn.status(), Some(Status::BadGateway) | None) {
                return conn;
            }

            log::debug!(
                "no app to reach (status={:?}, upstream_ready={}); serving starting-up page",
                conn.status(),
                self.upstream.is_ready()
            );
            conn.with_status(Status::ServiceUnavailable)
                .with_response_header(KnownHeaderName::ContentType, "text/html; charset=utf-8")
                .with_body(STARTING_UP_PAGE)
        }
    }

    /// Per-connection state for the live-reload websocket.
    #[derive(Clone)]
    struct WsState {
        events: Sender<Event>,
        editor: Option<String>,
        open_targets: Arc<Mutex<Vec<EditorTarget>>>,
        status: Arc<Mutex<Option<Event>>>,
    }

    /// Whichever of the two halves of the duplex connection produced something.
    enum Duplex {
        /// An event to push to the browser (None once the channel closes).
        Outgoing(Option<Event>),
        /// A message from the browser (None once the socket closes/errors).
        Incoming(Option<Message>),
    }

    pub fn run(
        host: String,
        port: u16,
        upstream: super::DynamicUpstream,
        events: Sender<Event>,
        editor: Option<String>,
        open_targets: Arc<Mutex<Vec<EditorTarget>>>,
        status: Arc<Mutex<Option<Event>>>,
    ) {
        let client = Client::new(ClientConfig::default().with_nodelay(true))
            .with_handler((ClientLogger::new(), Compression::new()));

        let state = WsState {
            events,
            editor,
            open_targets,
            status,
        };

        trillium_smol::config()
            .with_nodelay()
            .without_signals()
            .with_port(port)
            .with_host(&host)
            .run((
                Logger::new().with_formatter(log_format!(
                    "[proxy {upstream} {version} {method} {url} {response_time} {status} \
                     {body_len_human}]",
                    upstream = upstream.clone()
                )),
                HtmlRewriter::new(|| {
                    Settings::new_send().append_element_content_handler(element!("body", |el| {
                        el.append(
                            r#"<script src="/_dev_server.js"></script>"#,
                            ContentType::Html,
                        );
                        Ok(())
                    }))
                }),
                Router::new()
                    .get("/_dev_server.js", |conn: Conn| async move {
                        conn.with_response_header(
                            KnownHeaderName::ContentType,
                            "application/javascript; charset=utf-8",
                        )
                        .ok(include_str!("./dev_server.js"))
                    })
                    .get(
                        "/_dev_server.ws",
                        (State::new(state), WebSocket::new(live_reload)),
                    ),
                Proxy::new(client, upstream.clone())
                    .without_halting()
                    .with_websocket_upgrades(),
                StartingUpPage { upstream },
            ));
    }

    async fn live_reload(mut conn: WebSocketConn) {
        // Each connection gets its own receiver, so any number of browser tabs
        // can live-reload.
        let Some(state) = conn.state::<WsState>().cloned() else {
            return;
        };
        // Create the receiver before reading the sticky status, so no event
        // slips through the gap between the two.
        let mut rx = state.events.new_receiver();

        // Bring a freshly-connected browser up to date: if a build is in
        // progress or currently failing, show that immediately rather than
        // waiting for the next rebuild.
        let current = state.status.lock().unwrap().clone();
        if let Some(event) = current
            && conn.send_json(&event).await.is_err()
        {
            return;
        }

        loop {
            // Push build events out *and* accept "open in editor" requests over
            // the same socket. Scope the borrows so `conn` is free afterwards.
            let next = {
                let outgoing =
                    core::pin::pin!(async { Duplex::Outgoing(rx.recv_direct().await.ok()) });
                let incoming = core::pin::pin!(async {
                    Duplex::Incoming(conn.next().await.and_then(Result::ok))
                });
                future::or(outgoing, incoming).await
            };

            match next {
                Duplex::Outgoing(Some(event)) => {
                    if conn.send_json(&event).await.is_err() {
                        return;
                    }
                }
                Duplex::Incoming(Some(message)) => {
                    if message.is_close() {
                        return;
                    }
                    if let Ok(text) = message.to_text()
                        && let Ok(BrowserCommand::Open { id }) = serde_json::from_str(text)
                    {
                        // Resolve the id against the server's own table; the
                        // path is never taken from the browser.
                        let target = state.open_targets.lock().unwrap().get(id).cloned();
                        match (&state.editor, target) {
                            (Some(editor), Some(t)) => open_in_editor(
                                editor,
                                &t.path.to_string_lossy(),
                                t.line.max(1),
                                t.column.max(1),
                            ),
                            (None, _) => log::warn!(
                                "clicked a source link, but no editor is set ($EDITOR unset; pass \
                                 --editor)"
                            ),
                            (_, None) => log::warn!("ignoring open request for unknown id {id}"),
                        }
                    }
                }
                // Either side closing ends the connection.
                Duplex::Outgoing(None) | Duplex::Incoming(None) => return,
            }
        }
    }
}

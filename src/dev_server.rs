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
    collections::{HashMap, HashSet},
    env,
    fmt::Display,
    io::{self, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

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

    /// Extra arguments forwarded to `cargo build`, after a `--`
    ///
    /// Use this to select what gets built — cargo resolves the binary, so the
    /// dev server runs whatever it produces.
    ///
    /// Examples:
    ///    `trillium dev-server -- -p my-crate`
    ///    `trillium dev-server -- --bin worker --features dev`
    #[arg(last = true, verbatim_doc_comment)]
    cargo_args: Vec<String>,
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

        // The dev server adopts the user's public HOST/PORT; the app is moved
        // onto a private port (auto-allocated unless --app-port pins it).
        let upstream_port = self.app_port.unwrap_or_else(free_port);

        // Build command (reused for every rebuild). We ask cargo for JSON so it
        // tells us exactly which binary it produced and gives us structured
        // diagnostics, rather than reconstructing a target path ourselves.
        let mut build = Command::new("cargo");
        build
            .current_dir(&cwd)
            .args(["build", "--message-format=json-diagnostic-rendered-ansi"]);
        if self.release {
            build.arg("--release");
        }
        if let Some(example) = &self.example {
            build.args(["--example", example]);
        }
        build.args(&self.cargo_args);
        if !self.no_fast && !self.release {
            apply_fast_build(&mut build);
        }

        // Watch scope is small by default: the src of the crate cargo will
        // build (resolved from `-p`/the workspace), plus anything the user adds.
        let watches =
            resolve_watch_dirs(&cwd, &self.cargo_args, self.example.is_some(), &self.watch);

        print_banner(&self, upstream_port, &watches);

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
            thread::spawn(move || watch_loop(watches, cwd, tx));
        }

        // Shared with the websocket handler: locations the error overlay may
        // open, and the current build status to replay to new connections.
        let open_targets = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(Mutex::new(None));

        // Supervisor: owns the child process, builds, and restarts.
        {
            let broadcaster = broadcast_tx.clone();
            let app_host = self.app_host.clone();
            let signal = self.signal;
            let open_targets = open_targets.clone();
            let status = status.clone();
            thread::spawn(move || {
                Supervisor {
                    rx,
                    broadcaster,
                    build,
                    cwd,
                    signal,
                    app_host,
                    upstream_port,
                    exe: None,
                    child: None,
                    open_targets,
                    status,
                }
                .run()
            });
        }

        let editor = self.editor.clone().or_else(|| env::var("VISUAL").ok());
        let upstream = format!("http://{}:{}", self.app_host, upstream_port);
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
    upstream_port: u16,
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
            // A failure with no compiler diagnostics is cargo itself complaining
            // (e.g. an unknown `-p`). Surface its stderr as a single diagnostic.
            if terminal.trim().is_empty() {
                terminal = String::from_utf8_lossy(&output.stderr).into_owned();
            }
            io::stderr().write_all(terminal.as_bytes()).ok();
            if diagnostics.is_empty() {
                diagnostics.push(Diagnostic {
                    level: "error".into(),
                    message: "build failed".into(),
                    rendered: ansi_to_html::convert(&terminal).unwrap_or(terminal),
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
                "build produced {} binaries; running the last. Use `-- --bin <name>` to pick one.",
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
                     try `--example <name>` or `-- --bin <name>`"
                );
                false
            }
        }
    }

    fn spawn(&mut self) {
        let Some(exe) = self.exe.clone() else { return };
        // A fresh Command each time: the executable path can change between
        // builds (e.g. debug ↔ release, or a different selected target).
        let mut command = Command::new(exe);
        command
            .current_dir(&self.cwd)
            .env("PORT", self.upstream_port.to_string())
            .env("HOST", &self.app_host)
            .env("TRILLIUM_CLI_DEV_SERVER", "1");
        match command.spawn() {
            Ok(child) => {
                self.child = Some(child);
                wait_until_listening(&self.app_host, self.upstream_port);
            }
            Err(e) => log::error!("failed to start app: {e}"),
        }
    }

    /// Ask the running child to exit, then wait for it.
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = signal::kill(Pid::from_raw(child.id() as i32), self.signal);
            let _ = child.wait();
        }
    }

    fn run(mut self) {
        if self.build() {
            self.spawn();
        }

        loop {
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(()) => {
                    self.broadcast(Event::Rebuild);
                    if self.build() {
                        self.stop();
                        self.spawn();
                        if self.child.is_some() {
                            self.broadcast(Event::Restarted);
                        }
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
                        self.spawn();
                        if self.child.is_some() {
                            self.broadcast(Event::Restarted);
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// Block until something is listening on `host:port`, or give up after 10s.
fn wait_until_listening(host: &str, port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(addrs) = (host, port).to_socket_addrs() {
            for addr in addrs {
                if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                    log::info!("app is listening on {host}:{port}");
                    return;
                }
            }
        }
        if Instant::now() >= deadline {
            log::warn!("app did not start listening on {host}:{port} within 10s");
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Watch the given directories and send `()` on any change, debounced.
fn watch_loop(watches: Vec<PathBuf>, cwd: PathBuf, tx: mpsc::Sender<()>) {
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
        if events_rx.recv().is_err() {
            return;
        }
        // Coalesce a burst of events (one save touches many files) into a
        // single rebuild by draining until things go quiet.
        while events_rx.recv_timeout(Duration::from_millis(150)).is_ok() {}
        if tx.send(()).is_err() {
            return;
        }
    }
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

fn print_banner(server: &DevServer, upstream_port: u16, watches: &[PathBuf]) {
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
    println!(
        "  {}    http://{}:{}",
        "app".dimmed(),
        server.app_host,
        upstream_port
    );
    println!("  {}    {}", "watch".dimmed(), watches);
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
    use trillium::{Conn, KnownHeaderName, State};
    use trillium_client::Client;
    use trillium_html_rewriter::{
        HtmlRewriter,
        html::{Settings, element, html_content::ContentType},
    };
    use trillium_proxy::Proxy;
    use trillium_router::Router;
    use trillium_smol::ClientConfig;
    use trillium_websockets::{Message, WebSocket, WebSocketConn};

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
        upstream: String,
        events: Sender<Event>,
        editor: Option<String>,
        open_targets: Arc<Mutex<Vec<EditorTarget>>>,
        status: Arc<Mutex<Option<Event>>>,
    ) {
        let client = Client::new(ClientConfig::default().with_nodelay(true));
        let state = WsState {
            events,
            editor,
            open_targets,
            status,
        };

        trillium_smol::config()
            .without_signals()
            .with_port(port)
            .with_host(&host)
            .run((
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
                Proxy::new(client, &*upstream),
                HtmlRewriter::new(|| {
                    Settings::new_send().append_element_content_handler(element!("body", |el| {
                        el.append(
                            r#"<script src="/_dev_server.js"></script>"#,
                            ContentType::Html,
                        );
                        Ok(())
                    }))
                }),
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

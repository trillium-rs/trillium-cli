use crate::{
    client::{HttpVersion, parse_header, parse_method_case_insensitive},
    tls::{Tls, parse_url},
};
use clap::Parser;
use clap_verbosity_flag::Verbosity;
use colored::Colorize;
use hdrhistogram::Histogram;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::IsTerminal,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use trillium_client::{Body, Client, Error as ClientError, Method, Status, Url};
use trillium_http::{HttpConfig, HttpContext};

#[derive(Parser, Debug)]
pub struct BenchCli {
    /// target URL to benchmark
    #[arg(value_parser = parse_url)]
    url: Url,

    /// HTTP method
    #[arg(short = 'm', long, value_parser = parse_method_case_insensitive, default_value = "GET")]
    method: Method,

    /// number of concurrent connections (closed-loop) or initial pool size (open-loop)
    #[arg(short = 'c', long, default_value_t = 50)]
    connections: usize,

    /// total test duration (e.g. 10s, 1m, 30s500ms)
    ///
    /// mutually exclusive with --requests; default is 10s when neither is specified.
    #[arg(short = 'd', long, value_parser = parse_duration, conflicts_with = "requests")]
    duration: Option<Duration>,

    /// total number of requests to send (closed-loop only)
    #[arg(short = 'n', long, conflicts_with = "duration")]
    requests: Option<u64>,

    /// target rate in requests per second; switches to open-loop scheduling
    #[arg(short = 'r', long)]
    rate: Option<f64>,

    /// open-loop pacing strategy
    #[arg(long, value_enum, default_value_t = Pacing::Uniform)]
    pacing: Pacing,

    /// in open-loop mode, hard cap on simultaneous in-flight requests
    ///
    /// scheduled tickets that would exceed this cap are dropped and counted as saturation.
    #[arg(long)]
    max_concurrency: Option<usize>,

    /// discard statistics collected during this initial period
    #[arg(short = 'w', long, value_parser = parse_duration)]
    warmup: Option<Duration>,

    /// per-request timeout
    #[arg(long, value_parser = parse_duration)]
    timeout: Option<Duration>,

    /// request headers in KEY=VALUE form, repeatable
    #[arg(short = 'H', long, value_parser = parse_header)]
    headers: Vec<(String, String)>,

    /// path to a file to use as the request body
    #[arg(short = 'f', long)]
    file: Option<PathBuf>,

    /// inline request body string
    #[arg(short = 'b', long, conflicts_with = "file")]
    body: Option<String>,

    /// synthesize a zero-filled request body of the given size (e.g. 4kb, 1mb)
    #[arg(long, value_parser = parse_byte_size, conflicts_with_all = ["file", "body"])]
    body_size: Option<u64>,

    /// http version
    #[arg(long, value_enum, default_value_t)]
    http_version: HttpVersion,

    /// tls implementation
    #[arg(short, long, value_enum, default_value_t)]
    tls: Tls,

    /// disable http/1.1 connection reuse
    #[arg(long)]
    no_keepalive: bool,

    /// HttpConfig: initial response buffer length (bytes)
    #[arg(long, value_parser = parse_byte_size_usize)]
    response_buffer_len: Option<usize>,

    /// HttpConfig: maximum response buffer length under backpressure (bytes)
    #[arg(long, value_parser = parse_byte_size_usize)]
    response_buffer_max_len: Option<usize>,

    /// HttpConfig: max length of the http head (request line + headers)
    #[arg(long, value_parser = parse_byte_size_usize)]
    head_max_len: Option<usize>,

    /// HttpConfig: cooperative yield interval for the copy loop
    #[arg(long)]
    copy_loops_per_yield: Option<usize>,

    /// HttpConfig: maximum allowed received body length (bytes)
    #[arg(long, value_parser = parse_byte_size)]
    received_body_max_len: Option<u64>,

    /// emit the final report as JSON to stdout
    #[arg(long)]
    json: bool,

    /// write per-request timing data as CSV to this path
    #[arg(long)]
    csv: Option<PathBuf>,

    /// suppress the live progress display even when stdout is a tty
    #[arg(long)]
    no_progress: bool,

    /// log level (-v, -vv, -vvv)
    #[command(flatten)]
    verbose: Verbosity,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, clap::ValueEnum, Default)]
pub enum Pacing {
    /// fixed inter-arrival interval = 1 / rate
    #[default]
    Uniform,
    /// exponentially-distributed inter-arrival times with mean 1 / rate
    Poisson,
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| e.to_string())
}

fn parse_byte_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split_at = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (num, suffix) = s.split_at(split_at);
    let num: f64 = num
        .parse()
        .map_err(|_| format!("invalid number in size: `{s}`"))?;
    let multiplier: f64 = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        other => return Err(format!("unknown size suffix `{other}`")),
    };
    let bytes = num * multiplier;
    if !bytes.is_finite() || bytes < 0.0 {
        return Err(format!("invalid size `{s}`"));
    }
    Ok(bytes as u64)
}

fn parse_byte_size_usize(s: &str) -> Result<usize, String> {
    let bytes = parse_byte_size(s)?;
    usize::try_from(bytes).map_err(|_| format!("size `{s}` exceeds usize"))
}

#[derive(Debug, Default)]
struct Counters {
    completed: AtomicU64,
    succeeded: AtomicU64,
    errors_protocol: AtomicU64,
    errors_io: AtomicU64,
    errors_timeout: AtomicU64,
    errors_other: AtomicU64,
    saturation_drops: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    in_flight: AtomicU64,
}

#[derive(Debug)]
struct Stats {
    counters: Counters,
    statuses: Mutex<BTreeMap<u16, u64>>,
    ttfb: Mutex<Histogram<u64>>,
    total: Mutex<Histogram<u64>>,
    queue: Mutex<Histogram<u64>>,
    started_at: Mutex<Option<Instant>>,
    samples: Mutex<Option<Vec<Sample>>>,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    scheduled_offset_ms: f64,
    started_offset_ms: f64,
    queue_us: u64,
    ttfb_us: u64,
    total_us: u64,
    status: Option<u16>,
    bytes_received: u64,
    error: bool,
}

impl Stats {
    fn new(record_samples: bool) -> Self {
        let three_min_us = (3 * 60 * 1_000_000) as u64;
        let new_hist = || Histogram::<u64>::new_with_bounds(1, three_min_us, 3).unwrap();
        Self {
            counters: Counters::default(),
            statuses: Mutex::new(BTreeMap::new()),
            ttfb: Mutex::new(new_hist()),
            total: Mutex::new(new_hist()),
            queue: Mutex::new(new_hist()),
            started_at: Mutex::new(None),
            samples: Mutex::new(record_samples.then(Vec::new)),
        }
    }

    fn record(&self, sample: Sample) {
        let counters = &self.counters;
        counters.completed.fetch_add(1, Ordering::Relaxed);
        if sample.error {
            // categorized at error site
        } else {
            counters.succeeded.fetch_add(1, Ordering::Relaxed);
        }
        counters
            .bytes_received
            .fetch_add(sample.bytes_received, Ordering::Relaxed);

        if let Some(status) = sample.status {
            let mut s = self.statuses.lock().unwrap();
            *s.entry(status).or_default() += 1;
        }

        if !sample.error {
            let _ = self.ttfb.lock().unwrap().record(sample.ttfb_us.max(1));
            let _ = self.total.lock().unwrap().record(sample.total_us.max(1));
            let _ = self.queue.lock().unwrap().record(sample.queue_us.max(1));
        }

        if let Some(samples) = self.samples.lock().unwrap().as_mut() {
            samples.push(sample);
        }
    }

    fn record_error(&self, err: &ClientError, sample: Sample) {
        let bucket = match err {
            ClientError::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => {
                &self.counters.errors_timeout
            }
            ClientError::Io(_) => &self.counters.errors_io,
            _ => &self.counters.errors_protocol,
        };
        bucket.fetch_add(1, Ordering::Relaxed);
        self.record(Sample {
            error: true,
            ..sample
        });
    }
}

impl BenchCli {
    pub fn run(self) {
        async_global_executor::block_on(async move {
            self.run_async().await;
        });
    }

    async fn run_async(self) {
        env_logger::Builder::new()
            .parse_filters(&format!(
                "{},quinn=off,quinn_proto=off,rustls=off,tracing=off",
                self.verbose.log_level_filter()
            ))
            .init();

        let mode = self.mode();
        let body_bytes = self.load_body().await;
        let client = self.build_client();
        let stats = Arc::new(Stats::new(self.csv.is_some()));
        let stop = Arc::new(AtomicBool::new(false));
        let started_at = Instant::now();
        *stats.started_at.lock().unwrap() = Some(started_at);

        let warmup_until = self.warmup.map(|d| started_at + d);
        let deadline = match mode {
            Mode::ClosedDuration(d) | Mode::OpenDuration(_, d) => Some(started_at + d),
            _ => None,
        };

        let progress = if !self.no_progress && std::io::stderr().is_terminal() && !self.json {
            Some(spawn_progress(stats.clone(), stop.clone(), mode, deadline))
        } else {
            None
        };

        let request_ctx = Arc::new(RequestCtx {
            client,
            method: self.method,
            url: self.url.clone(),
            headers: self.headers.clone(),
            body_bytes,
            timeout: self.timeout,
            http_version: self.http_version.into(),
        });

        let run_ctx = RunCtx {
            request: request_ctx,
            stats: stats.clone(),
            stop: stop.clone(),
            started_at,
            warmup_until,
            deadline,
        };

        match mode {
            Mode::ClosedDuration(_) | Mode::ClosedRequests(_) => {
                run_closed_loop(self.connections, mode, &run_ctx).await;
            }
            Mode::OpenDuration(rate, _) | Mode::OpenUnbounded(rate) => {
                run_open_loop(rate, self.pacing, self.max_concurrency, &run_ctx).await;
            }
        }

        let elapsed = started_at.elapsed();
        if let Some(pb) = progress {
            pb.finish_and_clear();
        }
        let report = build_report(&stats, elapsed, self.warmup);

        if self.json {
            println!("{}", report.to_json());
        } else {
            print_report(&report);
        }

        if let Some(path) = &self.csv
            && let Some(samples) = stats.samples.lock().unwrap().as_ref()
            && let Err(e) = write_csv(path, samples)
        {
            eprintln!("failed to write csv: {e}");
        }
    }

    fn mode(&self) -> Mode {
        match (self.rate, self.duration, self.requests) {
            (Some(rate), Some(d), _) => Mode::OpenDuration(rate, d),
            (Some(rate), None, _) => Mode::OpenUnbounded(rate),
            (None, _, Some(n)) => Mode::ClosedRequests(n),
            (None, Some(d), None) => Mode::ClosedDuration(d),
            (None, None, None) => Mode::ClosedDuration(Duration::from_secs(10)),
        }
    }

    async fn load_body(&self) -> Option<Arc<Vec<u8>>> {
        if let Some(path) = &self.file {
            let bytes = async_fs::read(path)
                .await
                .unwrap_or_else(|e| panic!("could not read body file {path:?}: {e}"));
            Some(Arc::new(bytes))
        } else if let Some(body) = &self.body {
            Some(Arc::new(body.as_bytes().to_vec()))
        } else {
            self.body_size
                .map(|size| Arc::new(vec![0u8; size as usize]))
        }
    }

    fn build_client(&self) -> Client {
        let mut config = HttpConfig::default();
        if let Some(v) = self.response_buffer_len {
            config.set_response_buffer_len(v);
        }
        if let Some(v) = self.response_buffer_max_len {
            config.set_response_buffer_max_len(v);
        }
        if let Some(v) = self.head_max_len {
            config.set_head_max_len(v);
        }
        if let Some(v) = self.copy_loops_per_yield {
            config.set_copy_loops_per_yield(v);
        }
        if let Some(v) = self.received_body_max_len {
            config.set_received_body_max_len(v);
        }

        let context = HttpContext::default().with_config(config);
        let mut client = Client::from(self.tls).with_context(context);
        if let Some(timeout) = self.timeout {
            client.set_timeout(timeout);
        }
        if self.no_keepalive {
            client = client.without_keepalive();
        }
        client
    }
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    ClosedDuration(Duration),
    ClosedRequests(u64),
    OpenDuration(f64, Duration),
    OpenUnbounded(f64),
}

#[derive(Debug)]
struct RequestCtx {
    client: Client,
    method: Method,
    url: Url,
    headers: Vec<(String, String)>,
    body_bytes: Option<Arc<Vec<u8>>>,
    timeout: Option<Duration>,
    http_version: trillium_client::Version,
}

#[derive(Debug, Clone)]
struct RunCtx {
    request: Arc<RequestCtx>,
    stats: Arc<Stats>,
    stop: Arc<AtomicBool>,
    started_at: Instant,
    warmup_until: Option<Instant>,
    deadline: Option<Instant>,
}

async fn run_closed_loop(workers: usize, mode: Mode, run: &RunCtx) {
    let request_budget = match mode {
        Mode::ClosedRequests(n) => Some(Arc::new(AtomicU64::new(n))),
        _ => None,
    };

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let run = run.clone();
        let budget = request_budget.clone();
        handles.push(async_global_executor::spawn(async move {
            loop {
                if run.stop.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(d) = run.deadline
                    && Instant::now() >= d
                {
                    break;
                }
                if let Some(budget) = &budget {
                    let mut current = budget.load(Ordering::Relaxed);
                    let acquired = loop {
                        if current == 0 {
                            break false;
                        }
                        match budget.compare_exchange_weak(
                            current,
                            current - 1,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        ) {
                            Ok(_) => break true,
                            Err(actual) => current = actual,
                        }
                    };
                    if !acquired {
                        break;
                    }
                }
                run.stats.counters.in_flight.fetch_add(1, Ordering::Relaxed);
                let scheduled = Instant::now();
                execute_request(
                    &run.request,
                    &run.stats,
                    run.started_at,
                    run.warmup_until,
                    scheduled,
                    scheduled,
                )
                .await;
                run.stats.counters.in_flight.fetch_sub(1, Ordering::Relaxed);
            }
        }));
    }

    for h in handles {
        h.await;
    }
}

async fn run_open_loop(rate: f64, pacing: Pacing, max_concurrency: Option<usize>, run: &RunCtx) {
    if rate <= 0.0 {
        return;
    }
    let mean_interval = Duration::from_secs_f64(1.0 / rate);
    let mut next_at = Instant::now();
    let mut launched = Vec::new();

    loop {
        if run.stop.load(Ordering::Relaxed) {
            break;
        }
        if let Some(d) = run.deadline
            && Instant::now() >= d
        {
            break;
        }

        let now = Instant::now();
        if next_at > now {
            let wait = next_at - now;
            if let Some(d) = run.deadline {
                if next_at >= d {
                    break;
                }
                let until_deadline = d - now;
                if wait > until_deadline {
                    break;
                }
            }
            let stop = run.stop.clone();
            futures_lite::future::race(
                async {
                    async_io::Timer::after(wait).await;
                },
                async {
                    while !stop.load(Ordering::Relaxed) {
                        async_io::Timer::after(Duration::from_millis(50)).await;
                    }
                },
            )
            .await;
            if run.stop.load(Ordering::Relaxed) {
                break;
            }
        }

        let scheduled = next_at;
        let delta = match pacing {
            Pacing::Uniform => mean_interval,
            Pacing::Poisson => {
                let u = 1.0 - fastrand::f64();
                Duration::from_secs_f64(mean_interval.as_secs_f64() * -u.ln())
            }
        };
        next_at += delta;

        if let Some(cap) = max_concurrency
            && run.stats.counters.in_flight.load(Ordering::Relaxed) >= cap as u64
        {
            run.stats
                .counters
                .saturation_drops
                .fetch_add(1, Ordering::Relaxed);
            continue;
        }

        let run_inner = run.clone();
        run.stats.counters.in_flight.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        launched.push(async_global_executor::spawn(async move {
            execute_request(
                &run_inner.request,
                &run_inner.stats,
                run_inner.started_at,
                run_inner.warmup_until,
                scheduled,
                started,
            )
            .await;
            run_inner
                .stats
                .counters
                .in_flight
                .fetch_sub(1, Ordering::Relaxed);
        }));
    }

    for h in launched {
        h.await;
    }
}

async fn execute_request(
    ctx: &RequestCtx,
    stats: &Stats,
    test_started_at: Instant,
    warmup_until: Option<Instant>,
    scheduled: Instant,
    started: Instant,
) {
    let mut conn = ctx.client.build_conn(ctx.method, ctx.url.clone());
    conn.set_http_version(ctx.http_version);
    if let Some(timeout) = ctx.timeout {
        conn.set_timeout(timeout);
    }
    if !ctx.headers.is_empty() {
        conn.request_headers_mut().extend(ctx.headers.clone());
    }
    if let Some(bytes) = &ctx.body_bytes {
        conn.set_request_body(Body::from(bytes.as_ref().clone()));
        stats
            .counters
            .bytes_sent
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
    }

    let send_start = Instant::now();
    let mut sample = Sample {
        scheduled_offset_ms: (scheduled - test_started_at).as_secs_f64() * 1000.0,
        started_offset_ms: (started - test_started_at).as_secs_f64() * 1000.0,
        queue_us: (started - scheduled).as_micros() as u64,
        ttfb_us: 0,
        total_us: 0,
        status: None,
        bytes_received: 0,
        error: false,
    };

    match (&mut conn).await {
        Ok(()) => {
            let ttfb = send_start.elapsed();
            sample.ttfb_us = ttfb.as_micros() as u64;
            let status = conn.status().unwrap_or(Status::NotFound);
            sample.status = Some(u16::from(status));

            // drain the body to measure total time and received bytes
            let mut sink = futures_lite::io::sink();
            match futures_lite::io::copy(&mut conn.response_body(), &mut sink).await {
                Ok(n) => sample.bytes_received = n,
                Err(_) => {
                    stats.counters.errors_io.fetch_add(1, Ordering::Relaxed);
                    sample.error = true;
                }
            }
            sample.total_us = send_start.elapsed().as_micros() as u64;
        }
        Err(e) => {
            sample.total_us = send_start.elapsed().as_micros() as u64;
            if !is_warmup(warmup_until) {
                stats.record_error(&e, sample);
            }
            return;
        }
    }

    if !is_warmup(warmup_until) {
        stats.record(sample);
    }
}

fn is_warmup(warmup_until: Option<Instant>) -> bool {
    warmup_until.is_some_and(|until| Instant::now() < until)
}

#[derive(Debug)]
struct Report {
    elapsed: Duration,
    warmup: Option<Duration>,
    completed: u64,
    succeeded: u64,
    errors_io: u64,
    errors_timeout: u64,
    errors_protocol: u64,
    errors_other: u64,
    saturation_drops: u64,
    bytes_sent: u64,
    bytes_received: u64,
    statuses: BTreeMap<u16, u64>,
    ttfb: HistogramSnapshot,
    total: HistogramSnapshot,
    queue: HistogramSnapshot,
}

#[derive(Debug, Default, Clone, Copy)]
struct HistogramSnapshot {
    count: u64,
    min_us: u64,
    max_us: u64,
    mean_us: f64,
    stdev_us: f64,
    p50_us: u64,
    p75_us: u64,
    p90_us: u64,
    p95_us: u64,
    p99_us: u64,
    p999_us: u64,
}

impl HistogramSnapshot {
    fn from(h: &Histogram<u64>) -> Self {
        if h.is_empty() {
            return Self::default();
        }
        Self {
            count: h.len(),
            min_us: h.min(),
            max_us: h.max(),
            mean_us: h.mean(),
            stdev_us: h.stdev(),
            p50_us: h.value_at_quantile(0.50),
            p75_us: h.value_at_quantile(0.75),
            p90_us: h.value_at_quantile(0.90),
            p95_us: h.value_at_quantile(0.95),
            p99_us: h.value_at_quantile(0.99),
            p999_us: h.value_at_quantile(0.999),
        }
    }
}

fn build_report(stats: &Stats, elapsed: Duration, warmup: Option<Duration>) -> Report {
    let c = &stats.counters;
    Report {
        elapsed,
        warmup,
        completed: c.completed.load(Ordering::Relaxed),
        succeeded: c.succeeded.load(Ordering::Relaxed),
        errors_io: c.errors_io.load(Ordering::Relaxed),
        errors_timeout: c.errors_timeout.load(Ordering::Relaxed),
        errors_protocol: c.errors_protocol.load(Ordering::Relaxed),
        errors_other: c.errors_other.load(Ordering::Relaxed),
        saturation_drops: c.saturation_drops.load(Ordering::Relaxed),
        bytes_sent: c.bytes_sent.load(Ordering::Relaxed),
        bytes_received: c.bytes_received.load(Ordering::Relaxed),
        statuses: stats.statuses.lock().unwrap().clone(),
        ttfb: HistogramSnapshot::from(&stats.ttfb.lock().unwrap()),
        total: HistogramSnapshot::from(&stats.total.lock().unwrap()),
        queue: HistogramSnapshot::from(&stats.queue.lock().unwrap()),
    }
}

impl Report {
    fn to_json(&self) -> String {
        let h = |s: &HistogramSnapshot| {
            format!(
                r#"{{"count":{},"min_us":{},"max_us":{},"mean_us":{:.3},"stdev_us":{:.3},"p50_us":{},"p75_us":{},"p90_us":{},"p95_us":{},"p99_us":{},"p999_us":{}}}"#,
                s.count,
                s.min_us,
                s.max_us,
                s.mean_us,
                s.stdev_us,
                s.p50_us,
                s.p75_us,
                s.p90_us,
                s.p95_us,
                s.p99_us,
                s.p999_us
            )
        };
        let statuses = self
            .statuses
            .iter()
            .map(|(k, v)| format!(r#""{k}":{v}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"elapsed_secs":{:.6},"warmup_secs":{},"completed":{},"succeeded":{},"errors":{{"io":{},"timeout":{},"protocol":{},"other":{}}},"saturation_drops":{},"bytes_sent":{},"bytes_received":{},"throughput_rps":{:.3},"statuses":{{{}}},"ttfb_us":{},"total_us":{},"queue_us":{}}}"#,
            self.elapsed.as_secs_f64(),
            self.warmup
                .map_or("null".into(), |d| format!("{:.6}", d.as_secs_f64())),
            self.completed,
            self.succeeded,
            self.errors_io,
            self.errors_timeout,
            self.errors_protocol,
            self.errors_other,
            self.saturation_drops,
            self.bytes_sent,
            self.bytes_received,
            self.completed as f64 / self.elapsed.as_secs_f64().max(f64::EPSILON),
            statuses,
            h(&self.ttfb),
            h(&self.total),
            h(&self.queue),
        )
    }
}

fn print_report(r: &Report) {
    let header = |s: &str| println!("\n{}", s.bold().underline());
    let row = |label: &str, value: String| println!("  {:<22} {}", label.bright_blue(), value);

    header("Summary");
    row("Elapsed", format_duration(r.elapsed));
    if let Some(w) = r.warmup {
        row("Warmup discarded", format_duration(w));
    }
    row("Completed", r.completed.to_string());
    row("Succeeded", r.succeeded.to_string());
    row(
        "Throughput",
        format!(
            "{:.1} req/s",
            r.completed as f64 / r.elapsed.as_secs_f64().max(f64::EPSILON)
        ),
    );
    row("Bytes received", format_bytes(r.bytes_received));
    if r.bytes_sent > 0 {
        row("Bytes sent", format_bytes(r.bytes_sent));
    }
    row(
        "Receive throughput",
        format!(
            "{}/s",
            format_bytes(
                (r.bytes_received as f64 / r.elapsed.as_secs_f64().max(f64::EPSILON)) as u64
            )
        ),
    );

    if !r.statuses.is_empty() {
        header("Status codes");
        for (status, count) in &r.statuses {
            let label = format!("{status}");
            let colored = if (200..400).contains(status) {
                label.green()
            } else if (400..500).contains(status) {
                label.yellow()
            } else {
                label.bright_red()
            };
            println!("  {:<22} {}", colored.bold(), count);
        }
    }

    let total_errors = r.errors_io + r.errors_timeout + r.errors_protocol + r.errors_other;
    if total_errors > 0 || r.saturation_drops > 0 {
        header("Errors");
        if r.errors_io > 0 {
            row("io", r.errors_io.to_string());
        }
        if r.errors_timeout > 0 {
            row("timeout", r.errors_timeout.to_string());
        }
        if r.errors_protocol > 0 {
            row("protocol", r.errors_protocol.to_string());
        }
        if r.errors_other > 0 {
            row("other", r.errors_other.to_string());
        }
        if r.saturation_drops > 0 {
            row("saturation drops", r.saturation_drops.to_string());
        }
    }

    print_histogram("Latency (full response)", &r.total);
    print_histogram("Latency (TTFB)", &r.ttfb);
    if r.queue.count > 0 && r.queue.max_us > 1_000 {
        print_histogram("Open-loop queue wait", &r.queue);
    }
}

fn print_histogram(title: &str, s: &HistogramSnapshot) {
    if s.count == 0 {
        return;
    }
    println!("\n{}", title.bold().underline());
    let row = |label: &str, value: String| println!("  {:<8} {}", label.bright_blue(), value);
    row("min", format_us(s.min_us));
    row("mean", format_us_f(s.mean_us));
    row("p50", format_us(s.p50_us));
    row("p75", format_us(s.p75_us));
    row("p90", format_us(s.p90_us));
    row("p95", format_us(s.p95_us));
    row("p99", format_us(s.p99_us));
    row("p99.9", format_us(s.p999_us));
    row("max", format_us(s.max_us));
    row("stdev", format_us_f(s.stdev_us));
}

fn format_us(us: u64) -> String {
    format_us_f(us as f64)
}

fn format_us_f(us: f64) -> String {
    if us >= 1_000_000.0 {
        format!("{:.2} s", us / 1_000_000.0)
    } else if us >= 1_000.0 {
        format!("{:.2} ms", us / 1_000.0)
    } else {
        format!("{us:.0} µs")
    }
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 {
        format!("{:.0} ms", s * 1000.0)
    } else if s < 60.0 {
        format!("{s:.2} s")
    } else {
        let mins = (s / 60.0) as u64;
        let secs = s - (mins as f64) * 60.0;
        format!("{mins}m{secs:.1}s")
    }
}

fn format_bytes(bytes: u64) -> String {
    size::Size::from_bytes(bytes)
        .format()
        .with_base(size::Base::Base10)
        .to_string()
}

fn write_csv(path: &PathBuf, samples: &[Sample]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    writeln!(
        f,
        "scheduled_ms,started_ms,queue_us,ttfb_us,total_us,status,bytes_received,error"
    )?;
    for s in samples {
        writeln!(
            f,
            "{:.3},{:.3},{},{},{},{},{},{}",
            s.scheduled_offset_ms,
            s.started_offset_ms,
            s.queue_us,
            s.ttfb_us,
            s.total_us,
            s.status.map(|s| s.to_string()).unwrap_or_default(),
            s.bytes_received,
            u8::from(s.error)
        )?;
    }
    Ok(())
}

fn spawn_progress(
    stats: Arc<Stats>,
    stop: Arc<AtomicBool>,
    mode: Mode,
    deadline: Option<Instant>,
) -> ProgressBar {
    let pb = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr_with_hz(10));
    pb.set_style(
        ProgressStyle::with_template("{spinner} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "]),
    );

    let pb_clone = pb.clone();
    std::thread::spawn(move || {
        let mut last = Instant::now();
        let mut last_completed = 0u64;
        while !stop.load(Ordering::Relaxed) && !pb_clone.is_finished() {
            std::thread::sleep(Duration::from_millis(100));
            let now = Instant::now();
            let dt = (now - last).as_secs_f64();
            last = now;
            let completed = stats.counters.completed.load(Ordering::Relaxed);
            let recent_rps = ((completed - last_completed) as f64 / dt.max(1e-3)).round();
            last_completed = completed;
            let in_flight = stats.counters.in_flight.load(Ordering::Relaxed);
            let elapsed = stats
                .started_at
                .lock()
                .unwrap()
                .map_or(Duration::ZERO, |s| s.elapsed());

            let mut msg = String::new();
            let _ = write!(
                &mut msg,
                "{} {} | {} {} req | {} {:.0} req/s | {} {} | {}",
                "elapsed".dimmed(),
                format_duration(elapsed),
                "completed".dimmed(),
                completed,
                "rps".dimmed(),
                recent_rps,
                "in-flight".dimmed(),
                in_flight,
                progress_target(mode, deadline, completed),
            );

            let total = stats.total.lock().unwrap();
            if !total.is_empty() {
                let _ = write!(
                    &mut msg,
                    "\n{} p50 {} p90 {} p99 {} max {}",
                    "latency".dimmed(),
                    format_us(total.value_at_quantile(0.5)),
                    format_us(total.value_at_quantile(0.9)),
                    format_us(total.value_at_quantile(0.99)),
                    format_us(total.max())
                );
            }

            let total_errors = stats.counters.errors_io.load(Ordering::Relaxed)
                + stats.counters.errors_timeout.load(Ordering::Relaxed)
                + stats.counters.errors_protocol.load(Ordering::Relaxed)
                + stats.counters.errors_other.load(Ordering::Relaxed);
            if total_errors > 0 {
                let _ = write!(
                    &mut msg,
                    "\n{} {}",
                    "errors".bright_red().bold(),
                    total_errors
                );
            }

            pb_clone.set_message(msg);
            pb_clone.tick();
        }
    });
    pb
}

fn progress_target(mode: Mode, deadline: Option<Instant>, completed: u64) -> String {
    match mode {
        Mode::ClosedDuration(d) | Mode::OpenDuration(_, d) => {
            if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                format!(
                    "{} {} / {}",
                    "remaining".dimmed(),
                    format_duration(remaining),
                    format_duration(d)
                )
            } else {
                String::new()
            }
        }
        Mode::ClosedRequests(n) => format!("{} {} / {}", "progress".dimmed(), completed, n),
        Mode::OpenUnbounded(_) => String::new(),
    }
}

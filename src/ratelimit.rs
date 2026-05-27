use clap::Parser;
use trillium_ratelimit::{Quota, RateLimiter};

/// Per-client-network rate limiting, shared by `serve` and `proxy`.
///
/// Flattened into each subcommand's args. Disabled unless `--rate-limit` is
/// given, so it adds nothing to the common case.
#[derive(Parser, Debug, Clone, Copy)]
pub struct RateLimit {
    /// per-client-network request limit, e.g. 100/min, 10/s, 1000/h
    ///
    /// Enables rate limiting (off by default). Each client network is metered
    /// against its own quota; over-quota requests are rejected with 429 Too
    /// Many Requests and a Retry-After header, and every metered response
    /// carries RateLimit / RateLimit-Policy headers.
    #[arg(long = "rate-limit", value_name = "RATE", value_parser = parse_quota, verbatim_doc_comment, help_heading = "Rate limit")]
    quota: Option<Quota>,

    /// burst allowance above the sustained --rate-limit
    ///
    /// Permits short spikes before requests are held to the sustained rate.
    /// Defaults to the --rate-limit count.
    #[arg(
        long = "rate-limit-burst",
        requires = "quota",
        help_heading = "Rate limit"
    )]
    burst: Option<u64>,
}

impl RateLimit {
    /// The configured limiter, or `None` when `--rate-limit` was not given.
    ///
    /// `Option<Handler>` is itself a `Handler`, so a `None` drops straight out
    /// of the handler tuple instead of installing a pass-through.
    // Only `serve` and `proxy` consume `RateLimit` (clap-flattened args);
    // `gateway` configures rate limits via [`limiter_for`] from KDL instead.
    #[cfg(any(feature = "serve", feature = "proxy"))]
    pub fn limiter(self) -> Option<impl trillium::Handler> {
        self.quota.map(|quota| {
            let quota = match self.burst {
                Some(burst) => quota.allow_burst(burst),
                None => quota,
            };
            RateLimiter::by_network(quota)
        })
    }
}

/// Build a per-network rate limiter from a `COUNT/WINDOW` spec and optional
/// burst, for callers (like `gateway`) that configure rate limiting outside of
/// clap. Mirrors [`RateLimit::limiter`].
#[cfg(feature = "gateway")]
pub(crate) fn limiter_for(
    rate: &str,
    burst: Option<u64>,
) -> Result<impl trillium::Handler, String> {
    let quota = parse_quota(rate)?;
    let quota = match burst {
        Some(burst) => quota.allow_burst(burst),
        None => quota,
    };
    Ok(RateLimiter::by_network(quota))
}

/// Parse a `COUNT/WINDOW` rate spec, e.g. `100/min`, `10/s`, `1000/h`.
fn parse_quota(s: &str) -> Result<Quota, String> {
    let (count, window) = s
        .split_once('/')
        .ok_or_else(|| format!("expected COUNT/WINDOW, e.g. 100/min (got {s:?})"))?;

    let count = count
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("invalid request count {:?}", count.trim()))?;

    match window.trim().to_ascii_lowercase().as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => Ok(Quota::per_second(count)),
        "m" | "min" | "mins" | "minute" | "minutes" => Ok(Quota::per_minute(count)),
        "h" | "hr" | "hour" | "hours" => Ok(Quota::per_hour(count)),
        other => Err(format!(
            "unknown window {other:?}; use s, min, or h (e.g. 100/min)"
        )),
    }
}

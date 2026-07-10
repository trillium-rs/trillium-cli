//! Automatic directory listings, shared by the `serve` and `gateway`
//! subcommands.
//!
//! When [`StaticFileHandler`][trillium_static::StaticFileHandler] resolves a
//! request to a directory but has no index file to serve, it records a
//! [`ResolvedDirectory`][trillium_static::ResolvedDirectory] in conn state and
//! falls through without halting. [`DirectoryListing`] is placed after the file
//! handler: if that state is present it renders an HTML listing of the
//! directory; otherwise it leaves the conn untouched so the normal 404 path
//! applies.
//!
//! The page is built as a plain `String` — no template engine, no network
//! requests. Its one dependency is [`crate::assets`], which must be mounted
//! ahead of the file handler to serve the [`LISTING_CSS`] stylesheet the page
//! links.

use crate::assets::{BASE_CSS, LISTING_CSS, THEME_HEAD, THEME_TOGGLE};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use querystrong::QueryStrong;
use size::Size;
use std::{cmp::Ordering, fmt::Write, path::Path, time::SystemTime};
use trillium::{Conn, Handler, KnownHeaderName::ContentType};
use trillium_static::StaticConnExt;

/// Which column the listing is sorted by, chosen via the `sort` query param.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Name,
    Size,
    Modified,
}

impl SortKey {
    /// The `sort=` value used in links and parsed from the query string.
    fn param(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "modified",
        }
    }

    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("size") => SortKey::Size,
            Some("modified" | "date") => SortKey::Modified,
            _ => SortKey::Name,
        }
    }
}

/// Sort direction, chosen via the `order` query param.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    Asc,
    Desc,
}

impl Order {
    fn param(self) -> &'static str {
        match self {
            Order::Asc => "asc",
            Order::Desc => "desc",
        }
    }

    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("desc") => Order::Desc,
            _ => Order::Asc,
        }
    }

    fn flipped(self) -> Self {
        match self {
            Order::Asc => Order::Desc,
            Order::Desc => Order::Asc,
        }
    }
}

/// The sort state for one render, parsed from the request's query string.
#[derive(Debug, Clone, Copy)]
struct Sort {
    key: SortKey,
    order: Order,
}

impl Sort {
    fn from_query(querystring: &str) -> Self {
        let qs = QueryStrong::parse(querystring);
        Self {
            key: SortKey::parse(qs.get_str("sort")),
            order: Order::parse(qs.get_str("order")),
        }
    }
}

/// Renders an HTML directory index when the static file handler resolved a
/// directory it could not serve an index from.
///
/// `renderable` is an optional predicate (keyed on a lower-cased file
/// extension): when set, matching file rows get a `?render` link. `serve`
/// supplies it under `--render`; `gateway`, which has no render support, leaves
/// it `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectoryListing {
    renderable: Option<fn(&str) -> bool>,
}

impl DirectoryListing {
    /// A listing with no `?render` links.
    pub fn new() -> Self {
        Self::default()
    }

    /// A listing that links `?render` for entries `renderable` accepts.
    #[cfg(feature = "serve-render")]
    pub fn with_renderable(renderable: fn(&str) -> bool) -> Self {
        Self {
            renderable: Some(renderable),
        }
    }
}

impl Handler for DirectoryListing {
    async fn run(&self, conn: Conn) -> Conn {
        // Pull owned copies so the immutable borrows of `conn` end before we
        // build the response.
        let Some((fs_path, url_path, prefix)) = conn.resolved_directory().map(|dir| {
            (
                dir.path().to_path_buf(),
                request_path(&conn).to_string(),
                mount_prefix(&conn).to_string(),
            )
        }) else {
            return conn;
        };
        let sort = Sort::from_query(conn.querystring());

        // `read_dir` + per-entry `metadata` are blocking syscalls; keep them off
        // the async executor.
        let entries = match blocking::unblock(move || read_entries(&fs_path)).await {
            Ok(mut entries) => {
                sort_entries(&mut entries, sort);
                entries
            }
            Err(error) => {
                log::warn!("could not list {url_path}: {error}");
                return conn; // fall through to 404
            }
        };

        let body = render(&url_path, &prefix, &entries, sort, self.renderable);

        conn.with_response_header(ContentType, "text/html; charset=utf-8")
            .ok(body)
            .halt()
    }
}

/// The full request path, query stripped.
///
/// [`Conn::path`] is *router-relative* — under `gateway`'s `route "/docs/*"` a
/// request for `/docs/sub/` arrives here as `sub/`. Every link on the page is
/// absolute, so they must be built from the whole path instead.
fn request_path(conn: &Conn) -> &str {
    conn.path_and_query()
        .split_once('?')
        .map_or_else(|| conn.path_and_query(), |(path, _)| path)
}

/// The path prefix a router stripped before reaching this handler (`/docs` for
/// `route "/docs/*"`; empty for `serve`, which has no router).
///
/// The assets handler is mounted in the same stack, so it answers under the same
/// prefix — the stylesheet has to be linked relative to it, not from the origin
/// root.
///
/// Found by subtracting the router-relative [`Conn::path`] from the full request
/// path. Both ends are slash-normalized first: the full path of a directory
/// request keeps its trailing slash (`/docs/sub/`) while `path` does not
/// (`sub`), so they only line up once trimmed.
fn mount_prefix(conn: &Conn) -> &str {
    let full = request_path(conn).trim_end_matches('/');
    full.strip_suffix(conn.path().trim_matches('/'))
        .unwrap_or("")
        .trim_end_matches('/')
}

/// One row in the listing.
struct Entry {
    name: String,
    is_dir: bool,
    /// `None` for directories and for entries we could not stat.
    len: Option<u64>,
    modified: Option<SystemTime>,
}

/// Read the directory's entries, statting each for type, size, and mtime.
fn read_entries(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata().ok();
        let is_dir = meta.as_ref().is_some_and(std::fs::Metadata::is_dir);
        entries.push(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir,
            len: meta
                .as_ref()
                .filter(|_| !is_dir)
                .map(std::fs::Metadata::len),
            modified: meta.as_ref().and_then(|m| m.modified().ok()),
        });
    }
    Ok(entries)
}

/// Sort entries for display. Directories always group before files (they're
/// navigational); within each group, entries are ordered by the selected
/// column and direction, falling back to case-insensitive name for stability.
fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| {
        // Directories first, regardless of sort key or direction.
        if a.is_dir != b.is_dir {
            return b.is_dir.cmp(&a.is_dir);
        }
        let within = match sort.key {
            SortKey::Name => Ordering::Equal,
            SortKey::Size => a.len.cmp(&b.len),
            SortKey::Modified => a.modified.cmp(&b.modified),
        };
        let within = match sort.order {
            Order::Asc => within,
            Order::Desc => within.reverse(),
        };
        // Name is the final tiebreak; flip it too so a descending sort fully
        // reverses rather than leaving equal-keyed rows in ascending name order.
        let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
        within.then(match sort.order {
            Order::Asc => by_name,
            Order::Desc => by_name.reverse(),
        })
    });
}

/// Characters to percent-encode inside a single URL path segment. We keep the
/// unreserved set and `/` legible and encode everything that could break out of
/// an `href` or confuse a parser.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

fn encode_segment(segment: &str) -> impl std::fmt::Display + '_ {
    utf8_percent_encode(segment, SEGMENT)
}

/// The lower-cased extension of a file name (empty if none), for the renderable
/// predicate.
fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// HTML-escape text destined for an element body or attribute value.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Format a modification time as `YYYY-MM-DD HH:MM:SS` (UTC), or `—` if unknown.
fn format_modified(modified: Option<SystemTime>) -> String {
    match modified {
        Some(time) => humantime::format_rfc3339_seconds(time)
            .to_string()
            .replace('T', " ")
            .trim_end_matches('Z')
            .to_string(),
        None => "\u{2014}".to_string(),
    }
}

/// Build a sortable `<th>` whose link sorts by `key`. Clicking the already-active
/// column flips direction; an arrow marks the active column. The href is a
/// query-only relative link, so it preserves the current path.
fn header_cell(label: &str, key: SortKey, sort: Sort, extra_class: &str) -> String {
    let active = sort.key == key;
    let order = if active {
        sort.order.flipped()
    } else {
        Order::Asc
    };
    // Arrow points the way values grow reading top-to-bottom: ascending ↓,
    // descending ↑.
    let (aria, arrow) = match (active, sort.order) {
        (true, Order::Asc) => (" aria-sort=\"ascending\"", " \u{2193}"),
        (true, Order::Desc) => (" aria-sort=\"descending\"", " \u{2191}"),
        (false, _) => ("", ""),
    };
    let class = if active {
        format!("sortable active{extra_class}")
    } else {
        format!("sortable{extra_class}")
    };
    format!(
        "<th class=\"{class}\"{aria}><a href=\"?sort={}&amp;order={}\">{label}{arrow}</a></th>",
        key.param(),
        order.param(),
    )
}

/// Build the full HTML page for `url_path` (the full request path) and its
/// entries. `prefix` is the router-stripped mount prefix, used to reach the
/// stylesheet. `renderable`, when set, decides which file rows get a `?render`
/// link.
fn render(
    url_path: &str,
    prefix: &str,
    entries: &[Entry],
    sort: Sort,
    renderable: Option<fn(&str) -> bool>,
) -> String {
    // Absolute base for hrefs, always trailing-slashed so it works whether or
    // not the request path had a trailing slash.
    let base = if url_path.ends_with('/') {
        url_path.to_string()
    } else {
        format!("{url_path}/")
    };

    let title = format!("Index of {}", escape(url_path));
    let head = format!(
        "{}{}{}",
        header_cell("Name", SortKey::Name, sort, ""),
        header_cell("Size", SortKey::Size, sort, " size"),
        header_cell("Last modified", SortKey::Modified, sort, " modified"),
    );
    let mut rows = String::new();

    // Parent link, unless we're already at the root of what's mounted here —
    // under a router prefix that's `/docs/`, not `/`, and linking above it would
    // leave the routes this listing lives in.
    if base != format!("{prefix}/") {
        let parent = parent_path(&base);
        let _ = write!(
            rows,
            "<tr><td class=\"name\"><a \
             href=\"{parent}\">{FOLDER_ICON}<span>../</span></a></td><td class=\"size\"></td><td \
             class=\"modified\"></td></tr>"
        );
    }

    for entry in entries {
        let slash = if entry.is_dir { "/" } else { "" };
        let href = format!("{base}{}{slash}", encode_segment(&entry.name));
        let icon = if entry.is_dir { FOLDER_ICON } else { FILE_ICON };
        let size = match entry.len {
            Some(len) => Size::from_bytes(len).to_string(),
            None => "\u{2014}".to_string(),
        };
        // When rendering is enabled and the file is renderable, the name links to
        // the rendered view (that's what a reader almost always wants) and a
        // "view raw" link exposes the untransformed bytes. Everything else links
        // straight to the file, with no extra link.
        let renders = matches!(renderable, Some(is_renderable)
            if !entry.is_dir && is_renderable(&extension(&entry.name)));
        let (name_href, raw_link) = if renders {
            (
                format!("{href}?render"),
                format!("<a class=\"raw\" href=\"{href}\" title=\"raw\">view raw</a>"),
            )
        } else {
            (href, String::new())
        };
        let _ = write!(
            rows,
            "<tr><td class=\"name\"><a \
             href=\"{name_href}\">{icon}<span>{name}{slash}</span></a>{raw_link}</td><td \
             class=\"size\">{size}</td><td class=\"modified\">{modified}</td></tr>",
            name = escape(&entry.name),
            modified = format_modified(entry.modified),
        );
    }

    format!(
        "<!DOCTYPE html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title}</title>\n\
{THEME_HEAD}\n\
<link rel=\"stylesheet\" href=\"{prefix}{BASE_CSS}\">\n\
<link rel=\"stylesheet\" href=\"{prefix}{LISTING_CSS}\">\n\
</head>\n\
<body>\n\
{THEME_TOGGLE}\n\
<main>\n\
<h1>{title}</h1>\n\
<table>\n\
<thead><tr>{head}</tr></thead>\n\
<tbody>\n{rows}</tbody>\n\
</table>\n\
<footer>served by <a href=\"https://trillium.rs\">trillium</a></footer>\n\
</main>\n\
</body>\n\
</html>\n"
    )
}

/// Given a trailing-slashed absolute base like `/a/b/`, return its parent `/a/`.
fn parent_path(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(idx) => trimmed[..=idx].to_string(),
        None => "/".to_string(),
    }
}

/// Row icons. Inline rather than in `listing.css` because they're content, not
/// styling — the page emits one per row, and they're `fill`ed by the stylesheet.
const FOLDER_ICON: &str = "<svg viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M1.5 \
                           2.5h4l1.5 1.5h7.5v9h-13z\"/></svg>";
const FILE_ICON: &str =
    "<svg viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M3 1.5h6L13 5v9.5H3z\"/></svg>";

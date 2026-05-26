//! Automatic directory listings for the `serve` subcommand.
//!
//! When [`StaticFileHandler`][trillium_static::StaticFileHandler] resolves a
//! request to a directory but has no index file to serve, it records a
//! [`ResolvedDirectory`][trillium_static::ResolvedDirectory] in conn state and
//! falls through without halting. [`DirectoryListing`] is placed after the file
//! handler: if that state is present it renders a self-contained HTML listing of
//! the directory; otherwise it leaves the conn untouched so the normal 404 path
//! applies.
//!
//! The page is built as a plain `String` — no template engine, no external
//! assets, no network requests — so it works the moment the binary runs.

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
#[derive(Debug, Clone, Copy)]
pub struct DirectoryListing;

impl Handler for DirectoryListing {
    async fn run(&self, conn: Conn) -> Conn {
        // Pull owned copies so the immutable borrows of `conn` end before we
        // build the response.
        let Some((fs_path, url_path)) = conn
            .resolved_directory()
            .map(|dir| (dir.path().to_path_buf(), conn.path().to_string()))
        else {
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

        let body = render(&url_path, &entries, sort);

        conn.with_response_header(ContentType, "text/html; charset=utf-8")
            .ok(body)
            .halt()
    }
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
            len: meta.as_ref().filter(|_| !is_dir).map(std::fs::Metadata::len),
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
    let order = if active { sort.order.flipped() } else { Order::Asc };
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

/// Build the full HTML page for `url_path` (the request path) and its entries.
fn render(url_path: &str, entries: &[Entry], sort: Sort) -> String {
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

    // Parent link, unless we're already at the root.
    if base != "/" {
        let parent = parent_path(&base);
        let _ = write!(
            rows,
            "<tr><td class=\"name\"><a href=\"{parent}\">{FOLDER_ICON}<span>../</span></a></td>\
             <td class=\"size\"></td><td class=\"modified\"></td></tr>"
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
        let _ = write!(
            rows,
            "<tr><td class=\"name\"><a href=\"{href}\">{icon}<span>{name}{slash}</span></a></td>\
             <td class=\"size\">{size}</td><td class=\"modified\">{modified}</td></tr>",
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
<style>{STYLE}</style>\n\
</head>\n\
<body>\n\
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

const FOLDER_ICON: &str = "<svg viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M1.5 2.5h4l1.5 1.5h7.5v9h-13z\"/></svg>";
const FILE_ICON: &str = "<svg viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M3 1.5h6L13 5v9.5H3z\"/></svg>";

const STYLE: &str = "\
:root{color-scheme:light dark;--fg:#1a1a1a;--muted:#6b7280;--bg:#ffffff;--row:#f3f4f6;--border:#e5e7eb;--accent:#2563eb;--icon:#9ca3af;}\
@media(prefers-color-scheme:dark){:root{--fg:#e5e7eb;--muted:#9ca3af;--bg:#0b0d12;--row:#161a22;--border:#262b36;--accent:#60a5fa;--icon:#6b7280;}}\
*{box-sizing:border-box;}\
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;}\
main{max-width:880px;margin:0 auto;padding:2.5rem 1.25rem 4rem;}\
h1{font-size:1.15rem;font-weight:600;margin:0 0 1.25rem;word-break:break-all;}\
table{width:100%;border-collapse:collapse;}\
th{text-align:left;font-size:.75rem;text-transform:uppercase;letter-spacing:.05em;color:var(--muted);font-weight:600;padding:0 .75rem .5rem;border-bottom:1px solid var(--border);}\
th.sortable a{display:inline-flex;align-items:center;gap:.2rem;color:inherit;text-decoration:none;font:inherit;}\
th.sortable a:hover{color:var(--fg);}\
th.active{color:var(--fg);}\
td{padding:.45rem .75rem;border-bottom:1px solid var(--border);white-space:nowrap;}\
tr:hover td{background:var(--row);}\
td.name{width:100%;}\
td.size,th.size{text-align:right;font-variant-numeric:tabular-nums;color:var(--muted);}\
td.modified,th.modified{color:var(--muted);font-variant-numeric:tabular-nums;}\
a{display:flex;align-items:center;gap:.5rem;color:var(--accent);text-decoration:none;overflow:hidden;}\
a:hover span{text-decoration:underline;}\
a span{overflow:hidden;text-overflow:ellipsis;}\
svg{flex:none;width:1rem;height:1rem;fill:var(--icon);}\
footer{margin-top:1.5rem;font-size:.8rem;color:var(--muted);}\
footer a{display:inline;color:var(--muted);text-decoration:underline;}\
@media(max-width:520px){td.modified,th.modified{display:none;}}";

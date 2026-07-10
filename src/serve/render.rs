//! `?render`: turn a file the static handler just served into a viewable HTML
//! page — syntax-highlighted source, rendered markdown, or (as a universal
//! escape hatch) a JSON envelope.
//!
//! This handler is placed *after* [`StaticFileHandler`][trillium_static::StaticFileHandler],
//! so it never resolves paths itself: the file has already been read into the
//! response body and the content-type guessed. When `?render` is present on a
//! request that produced a body, we take that body back out
//! ([`Conn::take_response_body`]), read it to bytes, and replace it with a
//! rendered page. Anything else is left untouched for the normal file / listing
//! / 404 paths.
//!
//! Syntax highlighting uses [`two_face`]'s expanded syntax set (the stock
//! syntect bundle omits Rust, TOML, and much else) and is CPU-bound, so it runs
//! on the `blocking` pool the same way the directory listing's `read_dir` does.

use crate::assets::{BASE_CSS, MARKDOWN_CSS, THEME_HEAD, THEME_TOGGLE};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use pulldown_cmark::{
    CodeBlockKind, Event, Options, Parser as MarkdownParser, Tag, TagEnd, html::push_html,
};
use std::{path::Path, sync::LazyLock};
use syntect::{
    highlighting::{Theme, ThemeSet},
    html::{ClassStyle, ClassedHTMLGenerator, css_for_theme_with_class_style},
    parsing::{SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};
use trillium::{
    Conn, Handler,
    KnownHeaderName::{AcceptRanges, ContentType, Etag, LastModified},
    Status,
};
use trillium_static::StaticConnExt;

/// The expanded syntax set (Rust, TOML, and friends), loaded once.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
/// syntect's bundled themes; we use a light one by default and a dark one under
/// `prefers-color-scheme: dark`.
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);
/// The `<style>` block for highlighted code: token colors for both themes plus
/// the `<pre>` background/foreground, built once from [`THEMES`].
static CODE_CSS: LazyLock<String> = LazyLock::new(build_code_css);

/// Output format selected by the `render` query param: bare `?render` (or
/// `?render=html`) renders a page; `?render=json` returns a JSON envelope.
#[derive(Debug, Clone, Copy)]
enum Format {
    Html,
    Json,
}

/// Whether (and how) this request asked to be rendered. Parsed by hand rather
/// than through a query library so a valueless `?render` is reliably detected.
fn requested_format(querystring: &str) -> Option<Format> {
    let mut format = None;
    for pair in querystring.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "render" {
            format = Some(match value {
                "json" => Format::Json,
                _ => Format::Html,
            });
        }
    }
    format
}

/// Renders a served file as an HTML page (or JSON) when `?render` is present.
#[derive(Debug, Clone, Copy)]
pub struct Render;

impl Handler for Render {
    // `before_send`, not `run`: the static handler `halt`s when it serves a file
    // (`send_file` ends in `conn.ok(..)`), which skips every later handler's
    // `run`. `before_send` fires regardless of halt, so this is where we get to
    // see — and replace — the body the file handler produced.
    async fn before_send(&self, mut conn: Conn) -> Conn {
        let Some(format) = requested_format(conn.querystring()) else {
            return conn;
        };
        // A directory with no index left a `ResolvedDirectory` and no body — the
        // listing/404 path owns that, nothing to render here.
        if conn.resolved_directory().is_some() {
            return conn;
        }
        // Only transform a successfully served file body; leave 404s, ranges
        // (206), and empty responses alone.
        if conn.status() != Some(Status::Ok) || conn.response_body().is_none() {
            return conn;
        }

        let path = conn.path().to_string();
        let ext = Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let Some(body) = conn.take_response_body() else {
            return conn;
        };
        let bytes = match body.into_bytes().await {
            Ok(bytes) => bytes.into_owned(),
            Err(error) => {
                log::warn!("could not read {path} for rendering: {error}");
                return conn.with_status(Status::InternalServerError);
            }
        };

        match format {
            Format::Json => render_json(conn, &path, &ext, bytes),
            Format::Html => render_html(conn, &path, &ext, bytes).await,
        }
    }
}

/// Build the HTML page. Non-UTF-8 (binary) content can't become a text page, so
/// the original bytes are restored and served as-is; an unrecognized text type
/// is likewise served raw (use `?render=json` to force a structured view).
async fn render_html(conn: Conn, path: &str, ext: &str, bytes: Vec<u8>) -> Conn {
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // Restore the untouched body; the static handler's content-type stands.
        Err(error) => return conn.with_body(error.into_bytes()),
    };

    let title = display_path(path);
    // A relative link back to the untransformed file: the last path segment, so
    // it resolves against the current `…/name?render` URL to `…/name` regardless
    // of any router mount prefix.
    let raw_href = raw_href(path);
    let body = if is_markdown(ext) {
        let rendered = blocking::unblock(move || markdown_to_html(&text)).await;
        markdown_page(&title, &raw_href, &rendered)
    } else {
        let ext = ext.to_string();
        // The closure owns `text` (blocking::unblock needs `'static`); on a miss
        // it hands the text back so we can serve the file untouched.
        match blocking::unblock(move || highlight(&ext, &text).ok_or(text)).await {
            Ok(inner) => code_page(&title, &raw_href, &inner),
            // Unrecognized text type: serve the file as the static handler had it.
            Err(text) => return conn.with_body(text.into_bytes()),
        }
    };

    finalize(conn, "text/html; charset=utf-8", body)
}

/// A JSON envelope for the file — the escape hatch for types we can't render as
/// a page. Binary content is reported without a `content` field.
fn render_json(conn: Conn, path: &str, ext: &str, bytes: Vec<u8>) -> Conn {
    let value = match String::from_utf8(bytes) {
        Ok(text) => serde_json::json!({
            "path": path,
            "extension": ext,
            "bytes": text.len(),
            "content": text,
        }),
        Err(error) => serde_json::json!({
            "path": path,
            "extension": ext,
            "bytes": error.into_bytes().len(),
            "binary": true,
        }),
    };
    finalize(
        conn,
        "application/json; charset=utf-8",
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )
}

/// Replace the response body with the rendered `body` and set `content_type`,
/// dropping the headers the static handler set that no longer describe the
/// payload — the etag and range advertisement belonged to the original file
/// bytes, not this transformed page.
fn finalize(mut conn: Conn, content_type: &str, body: String) -> Conn {
    let headers = conn.response_headers_mut();
    headers.remove(Etag);
    headers.remove(LastModified);
    headers.remove(AcceptRanges);
    conn.with_response_header(ContentType, content_type.to_string())
        .with_body(body)
}

/// syntect-highlight `text` (dispatched by extension, falling back to a
/// first-line shebang match) into class-tagged `<span>`s. `None` when no syntax
/// matches, so the caller can serve the file raw instead. CPU-bound: call on
/// the blocking pool.
fn highlight(ext: &str, text: &str) -> Option<String> {
    let syntax = SYNTAXES
        .find_syntax_by_extension(ext)
        .or_else(|| SYNTAXES.find_syntax_by_first_line(text.lines().next().unwrap_or_default()))?;
    Some(spans(syntax, text))
}

/// Highlight `source` with `syntax` into class-tagged `<span>`s (the classes are
/// styled by [`CODE_CSS`]). Shared by the standalone code view and markdown's
/// fenced blocks.
fn spans(syntax: &SyntaxReference, source: &str) -> String {
    let mut generator =
        ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAXES, ClassStyle::Spaced);
    for line in LinesWithEndings::from(source) {
        // A highlight failure on one line shouldn't drop the rest; emit the line
        // un-tagged and carry on.
        let _ = generator.parse_html_for_line_which_includes_newline(line);
    }
    generator.finalize()
}

/// The markdown extensions we enable — GitHub-flavored-ish, plus smart quotes.
///
/// Fenced code blocks are intercepted and syntect-highlighted (reusing the same
/// syntaxes and CSS as the standalone code view) instead of being emitted as a
/// plain `<pre><code>`; everything else is left to pulldown's default rendering.
fn markdown_to_html(text: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_GFM;

    // Rewrite each code block's events (`Start(CodeBlock)`, `Text`…, `End`) into
    // a single `Html` event carrying the highlighted block. `code` holds the
    // language token and accumulated source while we're inside one.
    let mut events = Vec::new();
    let mut code: Option<(String, String)> = None;
    for event in MarkdownParser::new_ext(text, options) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => language_token(&info).to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            // Code content arrives as `Text` events while a block is open.
            Event::Text(text) if code.is_some() => {
                code.as_mut().unwrap().1.push_str(&text);
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, source)) = code.take() {
                    events.push(Event::Html(highlight_fenced(&lang, &source).into()));
                }
            }
            other => events.push(other),
        }
    }

    let mut html = String::new();
    push_html(&mut html, events.into_iter());
    html
}

/// The language token from a fence info string: the first word, split on commas
/// and whitespace so mdBook-style `rust,no_run` looks up as `rust`.
fn language_token(info: &str) -> &str {
    info.split(|c: char| c == ',' || c.is_whitespace())
        .find(|token| !token.is_empty())
        .unwrap_or_default()
}

/// Render one fenced code block: syntect-highlighted when the language resolves,
/// otherwise an escaped plain block. Wrapped like the standalone code view so it
/// picks up [`CODE_CSS`].
fn highlight_fenced(lang: &str, source: &str) -> String {
    let inner = match (!lang.is_empty())
        .then(|| SYNTAXES.find_syntax_by_token(lang))
        .flatten()
    {
        Some(syntax) => spans(syntax, source),
        None => escape(source),
    };
    format!("<pre class=\"code\"><code>{inner}</code></pre>")
}

fn is_markdown(ext: &str) -> bool {
    matches!(ext, "md" | "markdown" | "mdown" | "mkd" | "mkdn")
}

/// Whether a file with this (lower-cased) extension renders as a *page* —
/// markdown, or any type syntect can highlight. Used by the directory listing to
/// decide which rows get a `?render` link. Everything else can still be fetched
/// as `?render=json`, but that's not worth a link.
pub fn is_renderable(ext: &str) -> bool {
    is_markdown(ext) || SYNTAXES.find_syntax_by_extension(ext).is_some()
}

/// The request path shown as a page title, leading slash trimmed and escaped.
fn display_path(path: &str) -> String {
    escape(path.trim_start_matches('/'))
}

/// Characters to percent-encode in the "view raw" link's path segment — controls
/// plus anything that could break out of the `href` or read as a query.
const RAW_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'\'')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`');

/// A relative href to the raw file: the last path segment, percent-encoded. As a
/// relative link it resolves against the current `…/name?render` URL back to
/// `…/name`, dropping the query, whatever router prefix the request came in on.
fn raw_href(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    utf8_percent_encode(name, RAW_SEGMENT).to_string()
}

/// Wrap highlighted code spans in a full page. syntect already escapes the code.
/// The syntect token colors ([`CODE_CSS`]) are generated at runtime, so they're
/// inlined; the shared shell ([`BASE_CSS`]) is linked from [`page`]. The `wide`
/// body class drops the reading measure so source lines get room to breathe.
fn code_page(title: &str, raw_href: &str, inner: &str) -> String {
    page(
        title,
        "wide",
        raw_href,
        &format!("<style>{}</style>", *CODE_CSS),
        &format!("<pre class=\"code\"><code>{inner}</code></pre>"),
    )
}

/// Wrap rendered markdown (already HTML) in a full page. Links [`MARKDOWN_CSS`]
/// and inlines [`CODE_CSS`] after it, so syntect-highlighted fenced blocks are
/// colored and `pre.code` wins over the generic `.markdown pre` rule.
fn markdown_page(title: &str, raw_href: &str, article: &str) -> String {
    page(
        title,
        "",
        raw_href,
        &format!(
            "<link rel=\"stylesheet\" href=\"{MARKDOWN_CSS}\">\n<style>{}</style>",
            *CODE_CSS
        ),
        &format!("<article class=\"markdown\">{article}</article>"),
    )
}

/// The shared page shell. [`BASE_CSS`] (fonts + palette + layout) is always
/// linked; `head` carries any page-specific stylesheet links or inline `<style>`.
/// `body_class` selects a layout variant (`wide` for code); `raw_href` links the
/// colophon to the untransformed file.
fn page(title: &str, body_class: &str, raw_href: &str, head: &str, content: &str) -> String {
    format!(
        "<!DOCTYPE html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title}</title>\n\
{THEME_HEAD}\n\
<link rel=\"stylesheet\" href=\"{BASE_CSS}\">\n\
{head}\n\
</head>\n\
<body class=\"{body_class}\">\n\
{THEME_TOGGLE}\n\
<main>\n\
<h1>{title}</h1>\n\
<nav class=\"pagenav\"><a href=\"{raw_href}\">view raw</a></nav>\n\
{content}\n\
<footer>served by <a href=\"https://trillium.rs\">trillium</a></footer>\n\
</main>\n\
</body>\n\
</html>\n"
    )
}

/// Build the code `<style>` from both themes. `InspiredGitHub` is the light
/// theme; `base16-mocha.dark` the dark one — warm, so it sits on base.css's warm
/// dark page instead of clashing like a cool blue-grey theme would.
///
/// The dark rules are applied through the same three-way cascade as base.css's
/// palette: the system-dark default (unless the visitor forced light) *and* an
/// explicit `data-theme="dark"` from the toggle — so a manual dark choice on a
/// light OS re-colors the code too, instead of leaving a light block on a dark
/// page. The `:root{…}` wrappers rely on CSS nesting, so a bare `.comment{…}`
/// token rule inside becomes `:root[…] .comment`.
fn build_code_css() -> String {
    let light = &THEMES.themes["InspiredGitHub"];
    let dark = &THEMES.themes["base16-mocha.dark"];
    let light_tokens =
        css_for_theme_with_class_style(light, ClassStyle::Spaced).unwrap_or_default();
    let dark_tokens = css_for_theme_with_class_style(dark, ClassStyle::Spaced).unwrap_or_default();
    let (light_bg, light_fg) = theme_colors(light);
    let (dark_bg, dark_fg) = theme_colors(dark);
    format!(
        "pre.code{{background:{light_bg};color:{light_fg};padding:1rem \
         1.25rem;overflow:auto;border-radius:8px;font:13px/1.6 'IBM Plex \
         Mono',ui-monospace,SFMono-Regular,Menlo,monospace;}}\npre.code \
         code{{white-space:pre;}}\n{light_tokens}\n@media(prefers-color-scheme:dark){{:root:\
         not([data-theme=\"light\"]){{pre.code{{background:{dark_bg};color:{dark_fg};}}\\
         n{dark_tokens}\n}}}}\n:root[data-theme=\"dark\"]{{pre.code{{background:{dark_bg};color:\
         {dark_fg};}}\n{dark_tokens}\n}}"
    )
}

/// The background/foreground of a theme as `#rrggbb`, defaulting sensibly when a
/// theme leaves them unset.
fn theme_colors(theme: &Theme) -> (String, String) {
    let hex = |c: syntect::highlighting::Color| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b);
    (
        theme
            .settings
            .background
            .map(hex)
            .unwrap_or_else(|| "#ffffff".into()),
        theme
            .settings
            .foreground
            .map(hex)
            .unwrap_or_else(|| "#000000".into()),
    )
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

//! The stylesheets and web fonts for the pages this binary generates, baked in
//! at compile time.
//!
//! [`handler`] serves `src/assets/` — the authored stylesheets under `/_css/`
//! and the woff2 files under `/_fonts/` that they reference. Serving them as
//! real files rather than inlining a `<style>` blob into every response means the
//! browser caches them once, and it keeps the CSS in `.css` files where it can be
//! read and edited.
//!
//! Two things need it, independently: the directory listing
//! ([`crate::directory_listing`], via `serve --directory-listing` or the
//! gateway's `directory-listing` flag) links [`BASE_CSS`] + [`LISTING_CSS`], and
//! `serve --render`'s pages link [`BASE_CSS`] (+ [`MARKDOWN_CSS`] for markdown).
//! It is mounted whenever either is on, and both go through the same handler.
//!
//! Place it ahead of the file handler: a hit serves-and-halts, a miss falls
//! through to the user's files. That means `/_css` and `/_fonts` shadow any
//! same-named paths in the served directory.

use trillium::Handler;

/// The shared page shell — fonts, palette, layout, colophon header, footer.
/// Linked by both the directory listing and the `?render` pages, so the two read
/// as one product.
pub const BASE_CSS: &str = "/_css/base.css";

/// The directory listing's table styling, layered over [`BASE_CSS`].
pub const LISTING_CSS: &str = "/_css/listing.css";

/// Typography for rendered markdown, layered over [`BASE_CSS`].
#[cfg(feature = "serve-render")]
pub const MARKDOWN_CSS: &str = "/_css/markdown.css";

/// Inline `<head>` script that applies a saved light/dark choice before first
/// paint, so an explicit override never flashes the system theme. Goes ahead of
/// the stylesheet link in every generated page; a no-op if the visitor never
/// touched the toggle (the `prefers-color-scheme` media query then applies).
pub const THEME_HEAD: &str = "<script>(function(){try{var \
                              t=localStorage.getItem(\"trillium-theme\");if(t===\"dark\"||t===\"\
                              light\")document.documentElement.setAttribute(\"data-theme\",t);\
                              }catch(e){}})();</script>";

/// The corner theme toggle dropped into every generated page. It cycles three
/// states — auto (follow the OS), light, dark — and remembers the choice. The
/// icon reflects the chosen *state* (half-filled circle for auto, sun for light,
/// moon for dark), driven by the `data-theme` attribute in [`BASE_CSS`]; auto is
/// the absence of the attribute, so the OS preference drives the palette. The
/// handler stores `"system"`/`"light"`/`"dark"`; [`THEME_HEAD`] reapplies it.
pub const THEME_TOGGLE: &str =
    "<button class=\"theme-toggle\" type=\"button\" aria-label=\"Switch theme: auto, light, or \
     dark\" title=\"Switch theme: auto, light, dark\"><svg class=\"auto\" viewBox=\"0 0 16 16\" \
     aria-hidden=\"true\"><circle cx=\"8\" cy=\"8\" r=\"5.4\" fill=\"none\" \
     stroke=\"currentColor\" stroke-width=\"1.4\"/><path d=\"M8 2.6a5.4 5.4 0 0 1 0 \
     10.8z\"/></svg><svg class=\"sun\" viewBox=\"0 0 16 16\" aria-hidden=\"true\"><circle \
     cx=\"8\" cy=\"8\" r=\"3.2\"/><path d=\"M8 .6v2.1M8 13.3v2.1M.6 8h2.1M13.3 8h2.1M2.7 2.7l1.5 \
     1.5M11.8 11.8l1.5 1.5M13.3 2.7l-1.5 1.5M4.2 11.8l-1.5 1.5\" fill=\"none\" \
     stroke=\"currentColor\" stroke-width=\"1.3\" stroke-linecap=\"round\"/></svg><svg \
     class=\"moon\" viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M13.5 9.7A5.6 5.6 0 0 1 \
     6.3 2.5 5.6 5.6 0 1 0 13.5 9.7z\"/></svg></button><script>(function(){var \
     b=document.querySelector(\".theme-toggle\");if(!b)return;b.addEventListener(\"click\",\
     function(){var \
     r=document.documentElement,o=[\"system\",\"light\",\"dark\"],c;try{c=localStorage.getItem(\"\
     trillium-theme\");}catch(e){}if(c!==\"light\"&&c!==\"dark\")c=\"system\";var \
     n=o[(o.indexOf(c)+1)%3];if(n===\"system\")r.removeAttribute(\"data-theme\");else \
     r.setAttribute(\"data-theme\",n);try{localStorage.setItem(\"trillium-theme\",n);\
     }catch(e){}});})();</script>";

/// The embedded assets, paired with an [`Etag`][trillium_caching_headers::Etag]
/// so `If-None-Match` gets a 304 from the compile-time-baked etags. The etag
/// handler is paired here rather than mounted at the top of the stack so that
/// conditional requests for these assets are answered without imposing etag
/// handling on the user's own files.
pub fn handler() -> impl Handler {
    (
        trillium_caching_headers::Etag::new(),
        trillium_static_compiled::static_compiled!("./src/assets"),
    )
}

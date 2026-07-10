//! Live reload for `serve --render`.
//!
//! A recursive [`notify`] watcher on the served root broadcasts a tick whenever
//! a file changes or appears; an injected client script (see `live.js`) holds a
//! websocket open and reloads the page on each tick. Because the reload is
//! driven from the browser, a directory listing re-fetches and reflects new or
//! removed files for free — there's nothing file-specific to diff.
//!
//! The script is injected by an [`HtmlRewriter`] into any `text/html` response,
//! so it lights up plain HTML files, rendered pages, and directory listings
//! alike without touching the file handler.

use async_broadcast::Sender;
use futures_lite::{StreamExt, future};
use notify::{
    EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{CreateKind, ModifyKind, RemoveKind},
};
use std::{path::PathBuf, pin::pin, sync::mpsc, thread, time::Duration};
use trillium::{Conn, Handler, KnownHeaderName::ContentType, State};
use trillium_html_rewriter::{
    HtmlRewriter,
    html::{Settings, element, html_content::ContentType as HtmlContentType},
};
use trillium_router::Router;
use trillium_websockets::{WebSocket, WebSocketConn};

/// Build the live-reload handler: an [`HtmlRewriter`] that injects the client
/// script into HTML responses, plus routes serving that script and the reload
/// websocket. Spawns the filesystem watcher as a side effect.
///
/// Place this ahead of the file handler so its routes intercept
/// `/_serve_live.*`, and after compression so the rewriter runs on the
/// uncompressed body (both are `before_send` handlers; earlier in the tuple runs
/// later on the way out).
pub fn handler(root: PathBuf) -> impl Handler {
    // A tick channel: the watcher thread broadcasts `()`, each connected browser
    // holds its own receiver. Overflow-drop and don't-wait-for-a-receiver so a
    // burst of changes (or no browser at all) never blocks the watcher.
    let (mut sender, keepalive_rx) = async_broadcast::broadcast::<()>(8);
    sender.set_overflow(true);
    sender.set_await_active(false);

    let watch_sender = sender.clone();
    thread::spawn(move || {
        // Hold a receiver for the whole run so the channel never fully closes
        // between browser connections.
        let _keepalive = keepalive_rx;
        watch(root, watch_sender);
    });

    (
        HtmlRewriter::new(|| {
            Settings::new_send().append_element_content_handler(element!("body", |el| {
                el.append(
                    r#"<script src="/_serve_live.js"></script>"#,
                    HtmlContentType::Html,
                );
                Ok(())
            }))
        }),
        Router::new()
            .get("/_serve_live.js", |conn: Conn| async move {
                conn.with_response_header(ContentType, "application/javascript; charset=utf-8")
                    .ok(include_str!("live.js"))
            })
            .get(
                "/_serve_live.ws",
                (State::new(sender), WebSocket::new(reload)),
            ),
    )
}

/// One connected browser: forward every tick as a websocket message (the client
/// reloads on any message), and end the task when the socket closes.
async fn reload(mut conn: WebSocketConn) {
    let Some(sender) = conn.state::<Sender<()>>().cloned() else {
        return;
    };
    let mut ticks = sender.new_receiver();

    loop {
        // Race a tick against the socket closing; scope the borrows so `conn` is
        // free to send afterward.
        let event = {
            let tick = pin!(async { Event::Tick(ticks.recv_direct().await.is_ok()) });
            let socket = pin!(async {
                conn.next().await;
                Event::Closed
            });
            future::or(tick, socket).await
        };

        match event {
            // A change: tell the browser to reload.
            Event::Tick(true) => {
                if conn.send_string("reload".to_string()).await.is_err() {
                    return;
                }
            }
            // The channel closed, or the socket closed/errored: we're done.
            Event::Tick(false) | Event::Closed => return,
        }
    }
}

/// The outcome of the tick/socket race in [`reload`].
enum Event {
    /// A watcher tick arrived; `true` if the channel is still open.
    Tick(bool),
    /// The websocket produced no more messages (closed/errored).
    Closed,
}

/// Watch `root` recursively and broadcast a tick on each burst of real changes.
/// Runs on its own thread; returns only if the watcher can't start or the
/// channel closes.
fn watch(root: PathBuf, sender: Sender<()>) {
    let (events_tx, events_rx) = mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(events_tx, notify::Config::default()) {
        Ok(watcher) => watcher,
        Err(error) => {
            log::warn!("live reload: could not start file watcher: {error}");
            return;
        }
    };
    if let Err(error) = watcher.watch(&root, RecursiveMode::Recursive) {
        log::warn!("live reload: could not watch {}: {error}", root.display());
        return;
    }
    log::info!("live reload watching {}", root.display());

    loop {
        let Ok(first) = events_rx.recv() else {
            return;
        };
        // Coalesce a burst (one save can touch several files) into a single
        // reload; only broadcast if at least one event was a real change rather
        // than a metadata/access blip.
        let mut relevant = is_change(&first);
        while let Ok(event) = events_rx.recv_timeout(Duration::from_millis(100)) {
            relevant |= is_change(&event);
        }
        if relevant {
            let _ = sender.try_broadcast(());
        }
    }
}

/// Whether an event is a content change worth reloading for — a create, modify,
/// or remove. Access events (a plain read) and errors are ignored so merely
/// serving a file doesn't trigger a reload loop.
fn is_change(event: &notify::Result<notify::Event>) -> bool {
    let Ok(event) = event else { return false };
    matches!(
        event.kind,
        EventKind::Create(
            CreateKind::Any | CreateKind::File | CreateKind::Folder | CreateKind::Other
        ) | EventKind::Modify(
            ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Other
        ) | EventKind::Remove(
            RemoveKind::Any | RemoveKind::File | RemoveKind::Folder | RemoveKind::Other
        )
    )
}

(() => {
  const HOST_ID = "__trillium_dev_server__";
  let root, pillEl, overlayEl, sock;

  function ensureUI() {
    if (document.getElementById(HOST_ID)) return;
    const host = document.createElement("div");
    host.id = HOST_ID;
    // A shadow root keeps the overlay's styles isolated from the app (and the
    // app's styles from leaking in).
    root = host.attachShadow({ mode: "open" });
    root.innerHTML = `
      <style>
        :host { all: initial; }
        [hidden] { display: none !important; }
        .pill {
          position: fixed; bottom: 16px; right: 16px; z-index: 2147483647;
          font: 13px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace;
          background: #1e1e2e; color: #cdd6f4; padding: 8px 14px;
          border-radius: 999px; border: 1px solid #313244;
          box-shadow: 0 6px 20px rgba(0,0,0,.35);
          display: flex; align-items: center; gap: 9px;
        }
        .pill.ok { color: #a6e3a1; }
        .pill.warn { color: #f9e2af; }
        .dot { width: 8px; height: 8px; border-radius: 50%; background: currentColor; }
        .spin {
          width: 11px; height: 11px; border: 2px solid currentColor;
          border-top-color: transparent; border-radius: 50%;
          animation: spin .8s linear infinite;
        }
        @keyframes spin { to { transform: rotate(360deg); } }
        .overlay {
          position: fixed; inset: 0; z-index: 2147483646; overflow: auto;
          padding: 40px 24px; box-sizing: border-box;
          background: rgba(17,17,27,.86); backdrop-filter: blur(3px);
          font: 13px/1.55 ui-monospace, SFMono-Regular, Menlo, monospace;
          color: #cdd6f4;
        }
        .header { max-width: 1040px; margin: 0 auto 22px; display: flex; align-items: baseline; gap: 14px; }
        .count { font-size: 21px; font-weight: 700; color: #f38ba8; }
        .hint { color: #6c7086; font-size: 12px; }
        .card {
          max-width: 1040px; margin: 0 auto 16px; background: #181825;
          border: 1px solid #313244; border-radius: 10px; overflow: hidden;
        }
        .top {
          display: flex; align-items: center; gap: 10px;
          padding: 10px 14px; background: #11111b; border-bottom: 1px solid #313244;
        }
        .badge {
          font-size: 11px; font-weight: 700; text-transform: uppercase;
          letter-spacing: .04em; padding: 2px 8px; border-radius: 5px;
          background: #f38ba8; color: #11111b;
        }
        .badge.warning { background: #f9e2af; }
        .msg { font-weight: 600; color: #f5f5f5; }
        .loc { margin-left: auto; white-space: nowrap; }
        .loc a { color: #89b4fa; text-decoration: none; font-size: 12px; }
        .loc a:hover { text-decoration: underline; }
        pre { margin: 0; padding: 14px; overflow: auto; white-space: pre; }
      </style>
      <div class="pill" hidden></div>
      <div class="overlay" hidden></div>`;
    (document.body || document.documentElement).appendChild(host);
    pillEl = root.querySelector(".pill");
    overlayEl = root.querySelector(".overlay");

    // Clicking a source location asks the dev server to open it in $EDITOR,
    // over the same websocket — no editor-specific URL scheme needed. We send
    // only the diagnostic's id; the server maps it back to a path it chose.
    overlayEl.addEventListener("click", (e) => {
      const link = e.target.closest("a[data-id]");
      if (!link) return;
      e.preventDefault();
      if (sock && sock.readyState === WebSocket.OPEN) {
        sock.send(JSON.stringify({ type: "Open", id: Number(link.dataset.id) }));
      }
    });
  }

  function pill(html, kind) {
    ensureUI();
    overlayEl.hidden = true;
    pillEl.className = "pill" + (kind ? " " + kind : "");
    pillEl.innerHTML = html;
    pillEl.hidden = false;
  }

  function showErrors(diagnostics) {
    ensureUI();
    const n = diagnostics.length;
    const cards = diagnostics
      .map((d) => {
        const loc =
          d.id != null
            ? `<span class="loc"><a href="#" data-id="${d.id}">${escapeHtml(
                `${d.file}:${d.line}:${d.column}`
              )}</a></span>`
            : d.file != null
              ? `<span class="loc">${escapeHtml(`${d.file}:${d.line}:${d.column}`)}</span>`
              : "";
        return `<div class="card">
            <div class="top">
              <span class="badge ${d.level}">${escapeHtml(d.level)}</span>
              <span class="msg">${escapeHtml(d.message)}</span>
              ${loc}
            </div>
            <pre>${d.rendered}</pre>
          </div>`;
      })
      .join("");
    overlayEl.innerHTML =
      `<div class="header">
         <span class="count">${n} ${n === 1 ? "error" : "errors"}</span>
         <span class="hint">trillium dev-server · fix and save to reload</span>
       </div>` + cards;
    pillEl.hidden = true;
    overlayEl.hidden = false;
  }

  function escapeHtml(s) {
    return String(s).replace(
      /[&<>"]/g,
      (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]
    );
  }

  // Exposed for debugging: manually drive the overlay from the console.
  window._devServer = { pill, showErrors };

  function connect() {
    sock = window._devServerWebsocket = new WebSocket(
      `ws://${window.location.host}/_dev_server.ws`
    );

    sock.addEventListener("message", ({ data }) => {
      const message = JSON.parse(data);
      switch (message.type) {
        case "Rebuild":
          pill(`<span class="spin"></span> rebuilding…`);
          break;
        case "CompileError":
          showErrors(message.diagnostics || []);
          break;
        case "BuildSuccess":
          pill(`<span class="spin"></span> reloading…`, "ok");
          break;
        case "Restarted":
          window.location.reload();
          break;
        default:
          console.log(data);
      }
    });

    // The dev server itself restarts during development; reconnect so live
    // reload keeps working without a manual refresh.
    sock.addEventListener("close", () => {
      pill(`<span class="dot"></span> dev server offline`, "warn");
      setTimeout(connect, 1000);
    });
    sock.addEventListener("error", () => sock.close());
  }

  connect();
})();

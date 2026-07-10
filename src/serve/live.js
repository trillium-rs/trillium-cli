// Live reload for `trillium serve --render`. Opens a websocket back to the
// server; any message means "something changed on disk" — reload the page.
// Reconnects if the server restarts, so a restart of `trillium serve` doesn't
// leave the page stranded.
(() => {
  function connect() {
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    const sock = new WebSocket(`${scheme}://${location.host}/_serve_live.ws`);
    sock.addEventListener("message", () => location.reload());
    // The server went away (e.g. restarted); keep trying, and reload once it's
    // back so we pick up whatever changed while it was down.
    sock.addEventListener("close", () => setTimeout(connect, 1000));
    sock.addEventListener("error", () => sock.close());
  }
  connect();
})();

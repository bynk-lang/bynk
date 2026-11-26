# 1053 — The Durable Object sends the close before it runs `on close`

- **Status:** Accepted (v0.787.2)

**Context.** #3902 made `webSocketClose` run `on close` and then complete the close handshake (Decision D), and `webSocketError` close with `1011` after `on close` (Decision E). An `on close` that releases the connection (removing it from the room's `Map`) closes the held socket with no arguments, which sent a close frame with no status ahead of the handler's reply. The reply's `ws.close(code, reason)` then hit an already-closed socket and was swallowed, so a client that closed with `4002 "bye"` saw `1005`. #3902's smoke asserted only that the close completed and #4474 put the lost code down to the platform's auto-reply, which was wrong (#4491). #3903 hit the same ordering on the expiry path and sends the close first there, which the issue requires (the client must see `1008 authentication expired`).

**Decision.** `webSocketClosing` sends the close first (the echoed code and reason where the code can be sent, a bare `close()` otherwise; `1011 "error"` for `webSocketError`), then marks the attachment closed and runs `on close` once. The close is sent whether or not the handler faults, since it is already out. Where the platform has already replied (`web_socket_auto_reply_to_close`), the second `close` is ignored and a throw is swallowed. The expiry check's `1008` uses the same order.

**Consequences.** A client sees the code it closed with, or `1008`/`1011` where the server ended the connection, whatever `on close` does with the held connection. A workerd smoke where the client closes with `4002 "bye"` against an `on close` that removes the connection asserts the echo (`BYNK_REQUIRE_WORKERD=1`). No source change: `on close` is called with the same `CloseInfo`.

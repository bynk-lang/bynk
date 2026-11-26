---
title: WebSocket
---
A WebSocket service holds a long-lived connection to a client. Like HTTP, cron,
and queue handlers it is declared in a `service` inside a `context`, with the
protocol bound on the service header — `from websocket(in:, out:)` — naming the
**frame types** the client sends (`in`) and the server sends (`out`). The upgrade
**authenticates at the edge** via a `by` actor, and the handler is handed an owned
[`Connection[out]`](/reference/types/#connection) it must dispose.

For the worked chat-room, see the guide
[Handle a WebSocket connection](/book/guides/entry-points/websocket/). For the
`Connection[F]` **type** and its ownership (linearity) discipline, see
[Type system — Connection](/reference/types/#connection).

## Service form

```bynk,ignore
service <Name> from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant { … }
  on message (roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant { … }
  on close (roomId: RoomId, close: CloseInfo) -> Effect[()] by user: Participant { … }
}
```

- **Frame types:** `in` is the inbound frame type (delivered to `on message`),
  `out` is the type the server sends (`connection.send` and the held
  `Connection[out]`).
- **`on open` — exactly one.** The upgrade handshake. It **must** name its actor
  with `by` (there is no anonymous upgrade — a WebSocket, like an HTTP route, has
  no safe default actor), and it receives a fresh, **owned** `connection:
  Connection[out]` the framework supplies. The handler must **dispose** of it; the
  canonical disposal is transfer into an agent (`Room(roomId).join(…,
  connection)`), an undisposed connection being `bynk.held.leak`.
- **`on message`** — an inbound frame arrived (v0.106). Its parameters are the
  route params plus the decoded `frame: <in>`.
- **`on close` — exactly one, required.** The connection ended. `on open` always hands its
  connection to an agent's state, and the runtime cannot tell which store entry holds it, so
  nothing removes it when the client leaves: a service without `on close` is
  `bynk.ws.missing_close`. Release the stored connection here (via the owning agent), or a dead
  connection stays in the room and a later broadcast to it raises. Its leading parameters are
  the leading route params of `on open`, as for `on message`.
  - **`close: CloseInfo`** — an optional **trailing** parameter of the `bynk` record
    `CloseInfo { code: Int, reason: String, clean: Bool }` (`consumes bynk { CloseInfo }`),
    why the connection ended: the close frame's `code` and `reason` (`1000` normal, `1001`
    the client navigating away), and `clean`, whether the close handshake completed. An
    abnormal disconnection (`1006`, no close frame) is `reason: ""` and `clean: false`.
    A connection ended by a non-disconnection error is `CloseInfo { code: 1011, reason:
    "error", clean: false }`.

A connection's route params (e.g. `roomId`) are validated through their `.of`
constructors at the edge (`400` on failure), exactly as HTTP route params are.

## Authentication at the edge

The upgrade is authenticated **before** the connection is accepted — fail-closed,
exactly like an HTTP route. The WebSocket boundary admits `None` and `Bearer`
actors and **rejects `Signature`**: a browser `WebSocket` cannot set an
`Authorization` header, so a Bearer token is read from the **`Sec-WebSocket-Protocol`**
subprotocols the client offers: the marker `bynk.bearer`, then the token (a browser sets it
with `new WebSocket(url, ["bynk.bearer", token])`). The `101` response selects `bynk.bearer`
and never the token, so the credential is not echoed in a response header, where proxies and
logs may record it, and a browser, which fails a connection whose response selects none of the
subprotocols it offered, keeps the connection. On Workers the Worker verifies the token with the same
audited JWT verifier HTTP uses, runs any refinement-actor authorization predicate
(`403`), and only on success forwards the upgrade to the Durable Object.

A request that offers no token, or a bad one, is `401`. The old form, `new WebSocket(url,
[token])`, offering the token alone, is refused with `400` and a fixed message naming the pair:
it only worked for clients that do not enforce the standard, and a `401` would hide the cause.
An upgrade to a `None` actor offers no subprotocol and is answered with none.

## Sending and broadcasting

The agent that owns the connection sends with `connection.send(frame)`
(**non-consuming** — the binding stays owned) and ends it with `connection.close()`
(**consuming**). `connection.close(code, reason)` closes with a code and reason the client
receives; both are optional (`close(code)` is allowed, `close(reason)` is not):

- **`code`** is `1000` (a normal closure) or from `3000` to `4999`, the range the protocol
  leaves to applications. A literal outside that, or one of `1004`, `1005`, `1006` and `1015`
  (which are never sent), is `bynk.ws.bad_close_code`; a computed code outside it faults.
- **`reason`** is at most 123 bytes of UTF-8, the protocol's limit. A literal over that is
  `bynk.ws.bad_close_reason`; a computed reason is cut to 123 bytes on a character boundary.

To fan a frame out to many clients, hold the connections in a
`store Map[K, Connection[out]]` and iterate:

```bynk,fragment
agent Room {
  key id: RoomId
  store members: Set[UserId]
  store conns: Map[UserId, Connection[ServerFrame]]

  on call post(sender: UserId, text: String) -> Effect[()] {
    let _ <- conns.parForEach((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))
    ()
  }
}
```

- **`forEach`** broadcasts **sequentially**; **`parForEach`** broadcasts in
  **parallel**, so one slow or half-dead connection
  does not head-of-line-block the room — the production-correct form (v0.107).
  Every send is issued, then the call waits for all of them to settle; if sends
  fault it raises the **first fault in connection order**, after the rest have
  settled, as a `par` block does. A dead connection still raises (use
  `parTraverseAll` to gather per-connection `Result`s instead).
- The closure parameter `c` is a **borrowed** held binding: `send` is allowed, but
  a consuming op (`close` or transfer) on it is `bynk.held.consume_on_borrow`.
- **Exclude-self** filters on the sender's key (`u != sender`), **not** on the
  connection — `Connection` is non-comparable by design
  (`bynk.types.held_not_comparable`).

## The `TestConnection` model

On the **bundle** target there is no Durable Object: a connection is a
`TestConnection` — a capture-and-inspect channel that records every frame sent — so
a WebSocket service is fully developable and testable with no Cloudflare runtime.
A `TestConnection` exposes `sent()` (the frames sent to it), `closed()`, `closeCode()` and `closeReason()`.
In a `suite`, a case builds one with `TestConnection[F]()` and drives the
service by address:

```bynk,fragment
let tc = TestConnection[ServerFrame]()
let _ <- ChatGateway.open(tc, roomId) by Participant(alice)
expect tc.sent() == [ServerFrame { text: "welcome" }]
let _ <- ChatGateway.message(tc, ClientFrame { text: "hi" })
```

`open` supplies the route params and the actor identity; `message` and `close`
reuse them from the connection. `close(tc, info)` also sets the `CloseInfo` an `on close`
with a `close: CloseInfo` parameter receives; `close(tc)` gives it a normal, clean closure,
`CloseInfo { code: 1000, reason: "", clean: true }`. On the connection, `tc.closed()` is whether
the server closed it (`connection.close()`), and `tc.closeCode()` and `tc.closeReason()` are the
code and reason it closed with, as `Option`s (`None` when it gave none). See [Driving a WebSocket
service](/reference/testing/#websocket-address) for the addresses and the
tiers.

This is what makes the §20 chat-room assertable: two participants join, a message
fans out to both, one leaves, and the next message reaches only the other.

## Platform mapping

| Target | Connection | Notes |
|---|---|---|
| **bundle** (`--target bundle`) | `TestConnection` | capture-and-inspect; runs under Node, no Durable Object |
| **Workers** (`--target workers`) | hibernatable WebSocket in a Durable Object | the Worker authenticates at the edge and accepts the socket into the addressed DO |

On Workers the connection maps onto a Durable Object via the hibernatable-WebSocket
API: a `Connection` stored in agent state **survives hibernation** and is restored
when the agent is rehydrated — a platform-supplied guarantee the language relies on
but does not implement. The hosting DO is resolved statically from the single
connection transfer the `on open` makes; a zero/multiple/non-routable transfer
shape is `bynk.ws.open_transfer_shape`. The Worker addresses that DO before the
`on open` body runs, so the transfer's key must be one it can evaluate on its own:
a parameter of `on open`, a `const` of the context, or a literal
(`Room(roomId)`, `Room(LOBBY)`, `Room("lobby")`). A key bound in the body or
computed from the actor's identity is `bynk.ws.open_transfer_key`.

The Durable Object's `webSocketClose` runs the service's `on close` with the client's
`CloseInfo`, then **completes the close handshake**: it replies with the client's code and
reason when that code can be sent (`1000` to `4999`, except `1004`, `1005`, `1006` and `1015`),
and with a bare `close()` otherwise (an abnormal `1006` has no close frame to acknowledge). The
reply is sent even if `on close` faults, and is harmless where the platform replies
automatically (`web_socket_auto_reply_to_close`), so the handshake always completes whichever
way the platform is configured. `webSocketError` handles a non-disconnection error: it logs the
error's class name (never a frame's contents), closes the socket with `1011` and runs `on
close` with `CloseInfo { code: 1011, reason: "error", clean: false }`, so the agent releases
the connection exactly once; the `webSocketClose` that follows replies but does not run it
again.

## Diagnostics

| Code | When |
|---|---|
| `bynk.held.leak` | a connection is left undisposed when its scope exits |
| `bynk.held.use_after_consume` | a connection is used after `close`/transfer |
| `bynk.held.branch_divergence` | `if`/`match` branches dispose inconsistently |
| `bynk.test.ws_needs_in_process` | a `system` case drives a WebSocket service by address |
| `bynk.test.ws_lifecycle_by` | a case's `message`/`close` address carries a `by` (the principal is given at `open`) |
| `bynk.held.consume_on_borrow` | a consuming op on a borrowed connection (e.g. in a broadcast closure) |
| `bynk.types.held_not_comparable` | a `Connection` compared with `==` |
| `bynk.ws.missing_close` | a `from websocket` service has no `on close` |
| `bynk.ws.bad_close_code` | a literal `connection.close` code outside `1000` and `3000`-`4999` |
| `bynk.ws.bad_close_reason` | a literal `connection.close` reason over 123 bytes of UTF-8 |
| `bynk.ws.open_transfer_shape` | the `on open` does not transfer the connection to exactly one routable agent |
| `bynk.ws.open_transfer_key` | on Workers, the transfer's key is not an `on open` parameter, a context `const` or a literal |
| `bynk.target.websocket_workers_unsupported` | (historical) retired once the Workers wire path landed |

## Related

- Guide: [Handle a WebSocket connection](/book/guides/entry-points/websocket/).
- Reference: [Type system — Connection](/reference/types/#connection) and
  [Stream](/reference/types/#stream).
- Reference: [HTTP](/reference/http/) — the sibling request/response protocol, including
  [streamed responses](/reference/http/#streamed-responses).

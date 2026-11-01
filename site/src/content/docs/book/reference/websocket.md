---
title: WebSocket
---
A WebSocket service holds a long-lived connection to a client. Like HTTP, cron,
and queue handlers it is declared in a `service` inside a `context`, with the
protocol bound on the service header — `from websocket(in:, out:)` — naming the
**frame types** the client sends (`in`) and the server sends (`out`). The upgrade
**authenticates at the edge** via a `by` actor, and the handler is handed an owned
[`Connection[out]`](/book/reference/types/#connection) it must dispose.

For the worked chat-room, see the guide
[Handle a WebSocket connection](/book/guides/entry-points/websocket/). For the
`Connection[F]` **type** and its ownership (linearity) discipline, see
[Type system — Connection](/book/reference/types/#connection).

## Service form

```bynk,ignore
service <Name> from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant { … }
  on message (roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant { … }
  on close (roomId: RoomId) -> Effect[()] by user: Participant { … }
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
- **`on close`** — the connection ended (v0.106). Dispose the stored connection
  (here, via the owning agent).

A connection's route params (e.g. `roomId`) are validated through their `.of`
constructors at the edge (`400` on failure), exactly as HTTP route params are.

## Authentication at the edge

The upgrade is authenticated **before** the connection is accepted — fail-closed,
exactly like an HTTP route. The WebSocket boundary admits `None` and `Bearer`
actors and **rejects `Signature`**: a browser `WebSocket` cannot set an
`Authorization` header, so a Bearer token is read from the first
**`Sec-WebSocket-Protocol`** subprotocol element (a browser sets it with `new
WebSocket(url, [token])`). On Workers the Worker verifies the token with the same
audited JWT verifier HTTP uses, runs any refinement-actor authorization predicate
(`403`), and only on success forwards the upgrade to the Durable Object.

## Sending and broadcasting

The agent that owns the connection sends with `connection.send(frame)`
(**non-consuming** — the binding stays owned) and ends it with `connection.close()`
(**consuming**). To fan a frame out to many clients, hold the connections in a
`store Map[K, Connection[out]]` and iterate:

```bynk,fragment
agent Room {
  key id: RoomId
  store members: Set[UserId]
  store conns: Map[UserId, Connection[ServerFrame]]

  on call post(sender: UserId, text: String) -> Effect[()] {
    let _ <- conns.parTraverse((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))
    ()
  }
}
```

- **`forEach`** broadcasts **sequentially**; **`parTraverse`** broadcasts in
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
A `TestConnection` exposes `.sent` (the array of frames sent to it) and `.closed`.
A handler is driven by calling the service's emitted handler with the connection,
the route params, and the actor identity:

```ts
const tc = new TestConnection<{ text: string }>();
await ChatGateway.open(tc, roomId, { identity: alice });
// tc.sent[0].text === "welcome"
await ChatGateway.message(tc, roomId, { text: "hi" }, { identity: alice });
```

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
shape is `bynk.ws.open_transfer_shape`.

## Diagnostics

| Code | When |
|---|---|
| `bynk.held.leak` | a connection is left undisposed when its scope exits |
| `bynk.held.use_after_consume` | a connection is used after `close`/transfer |
| `bynk.held.branch_divergence` | `if`/`match` branches dispose inconsistently |
| `bynk.held.consume_on_borrow` | a consuming op on a borrowed connection (e.g. in a broadcast closure) |
| `bynk.types.held_not_comparable` | a `Connection` compared with `==` |
| `bynk.ws.open_transfer_shape` | the `on open` does not transfer the connection to exactly one routable agent |
| `bynk.target.websocket_workers_unsupported` | (historical) retired once the Workers wire path landed |

## Related

- Guide: [Handle a WebSocket connection](/book/guides/entry-points/websocket/).
- Reference: [Type system — Connection](/book/reference/types/#connection) and
  [Stream](/book/reference/types/#stream).
- Reference: [HTTP](/book/reference/http/) — the sibling request/response protocol, including
  [streamed responses](/book/reference/http/#streamed-responses).

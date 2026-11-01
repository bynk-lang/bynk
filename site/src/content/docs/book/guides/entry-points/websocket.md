---
title: Handle a WebSocket connection
---
**Goal:** accept a long-lived WebSocket connection, authenticate it at the edge,
hand it to an agent, and broadcast messages to every client in a room — the §20
chat-room, end to end.

WebSocket handlers go in a `service` inside a `context`. The protocol is bound on
the service header — `from websocket(in:, out:)` — naming the **frame types** the
client sends (`in`) and the server sends (`out`). Unlike HTTP, the connection
outlives the handler: `on open` is handed an owned
[`Connection`](/book/reference/types/#connection) that it must **dispose** of,
the canonical disposal being transfer into an agent that holds it.

## Open a connection

```bynk,fragment
context chat

type RoomId = opaque String
type UserId = opaque String
type ServerFrame = { text: String }
type ClientFrame = { text: String }

actor Participant { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- connection.send(ServerFrame { text: "welcome" })
    let _ <- Room(roomId).join(user.identity, connection)
    ()
  }
}
```

The upgrade **authenticates at the edge** through the `by` clause — exactly like an
HTTP route, and fail-closed: there is no anonymous upgrade, so `on open` *must*
name its actor. (A browser `WebSocket` cannot set an `Authorization` header, so a
`Bearer` token rides the `Sec-WebSocket-Protocol` subprotocol; the boundary admits
`None`/`Bearer` and rejects `Signature`.) Only on success is the handler run with a
fresh, **owned** `connection: Connection[ServerFrame]`.

That connection is a [held resource](/book/reference/types/#connection): it has
exactly one owner and must be disposed before the handler returns. Here it is
**transferred into the `Room` agent** (`Room(roomId).join(…, connection)`); leaving
it undisposed is a compile error (`bynk.held.leak`).

## Hold connections in an agent

The agent stores each connection in a `Map` keyed by user, alongside the room's
membership:

```bynk,fragment
agent Room {
  key id: RoomId
  store members: Set[UserId]
  store conns: Map[UserId, Connection[ServerFrame]]

  on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] {
    let _ <- members.add(u)
    let _ <- conns.put(u, conn)
    ()
  }

  on call leave(u: UserId) -> Effect[()] {
    let _ <- members.remove(u)
    let _ <- conns.remove(u)
    ()
  }
}
```

A `Connection` may be stored **only** in `Cell[Option[Connection]]` or
`Map[K, Connection]` — `put` takes ownership and `remove` removes-and-closes; a
`Set`/`Log`/`Cache` rejects it.

## Receive inbound frames and broadcast

Inbound frames arrive through `on message`; `on close` fires when the client
disconnects. Each delegates to the room agent, which broadcasts over its held map:

```bynk,fragment
service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- connection.send(ServerFrame { text: "welcome" })
    let _ <- Room(roomId).join(user.identity, connection)
    ()
  }

  on message (roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).post(user.identity, frame.text)
    ()
  }

  on close (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).leave(user.identity)
    ()
  }
}
```

```bynk,fragment
  on call post(sender: UserId, text: String) -> Effect[()] {
    let _ <- conns.parTraverse((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))
    ()
  }
```

`parTraverse` sends to every connection **in parallel** (so one slow client cannot
stall the room, and it waits for every send to settle before it raises a fault); `forEach` is the sequential sibling. In the broadcast closure each
`c` is **borrowed** — you may `send` to it, but closing or transferring it is
`bynk.held.consume_on_borrow`. To broadcast to everyone *but* the sender, filter on
the sender's key (`u != sender`); a `Connection` cannot be compared with `==`
(`bynk.types.held_not_comparable`).

## Test it with no Cloudflare runtime

On the **bundle** target a connection is a `TestConnection` — it captures every
frame sent (`.sent`) and whether it was closed (`.closed`) — so the whole flow runs
under Node. Drive a handler by passing the connection, the route params, and the
actor identity:

```ts
const tcA = new TestConnection<{ text: string }>();
const tcB = new TestConnection<{ text: string }>();

await ChatGateway.open(tcA, roomId, { identity: alice });   // tcA.sent[0].text === "welcome"
await ChatGateway.open(tcB, roomId, { identity: bob });

await ChatGateway.message(tcA, roomId, { text: "hello room" }, { identity: alice });
// both tcA.sent and tcB.sent now end with { text: "hello room" }

await ChatGateway.close(tcB, roomId, { identity: bob });
await ChatGateway.message(tcA, roomId, { text: "after leave" }, { identity: alice });
// only tcA receives it — bob has left
```

## Build and run

On `--target workers` the upgrade is authenticated in the Worker and the socket is
accepted into a Durable Object using the hibernatable-WebSocket API — a stored
`Connection` survives hibernation and is restored on rehydration. On
`--target bundle` it runs against `TestConnection`. See
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).

## The complete example

Putting the pieces together — the §20 chat-room as one compiling program:

```bynk
context chat

type RoomId = opaque String
type UserId = opaque String
type ServerFrame = { text: String }
type ClientFrame = { text: String }

actor Participant { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- connection.send(ServerFrame { text: "welcome" })
    let _ <- Room(roomId).join(user.identity, connection)
    ()
  }

  on message (roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).post(user.identity, frame.text)
    ()
  }

  on close (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).leave(user.identity)
    ()
  }
}

agent Room {
  key id: RoomId
  store members: Set[UserId]
  store conns: Map[UserId, Connection[ServerFrame]]

  on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] {
    let _ <- members.add(u)
    let _ <- conns.put(u, conn)
    ()
  }

  on call leave(u: UserId) -> Effect[()] {
    let _ <- members.remove(u)
    let _ <- conns.remove(u)
    ()
  }

  on call post(sender: UserId, text: String) -> Effect[()] {
    let _ <- conns.parTraverse((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))
    ()
  }
}
```

## Related

- Reference: [WebSocket](/book/reference/websocket/) — the full handler surface,
  broadcast, authentication, and diagnostics.
- Reference: [Type system — Connection](/book/reference/types/#connection) — the
  held-resource type and its linearity discipline.
- Guide: [Handle an HTTP request](/book/guides/entry-points/http/) — the sibling request/response protocol.

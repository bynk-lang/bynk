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
[`Connection`](/reference/types/#connection) that it must **dispose** of,
the canonical disposal being transfer into an agent that holds it.

## Open a connection

```bynk,fragment
context chat

type RoomId = opaque String

type UserId = opaque String

type ServerFrame = {
	text: String,
}

type ClientFrame = {
	text: String,
}

actor Participant { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
	on open(roomId: RoomId) -> Effect[()] by user: Participant {
		let _ <- connection.send(ServerFrame { text: "welcome" })
		let _ <- Room(roomId).join(user.identity, connection)
	}
}
```

The upgrade **authenticates at the edge** through the `by` clause — exactly like an
HTTP route, and fail-closed: there is no anonymous upgrade, so `on open` *must*
name its actor. (A browser `WebSocket` cannot set an `Authorization` header, so a
`Bearer` token rides the `Sec-WebSocket-Protocol` subprotocols, as the pair
`new WebSocket(url, ["bynk.bearer", token])`; an `Oidc` token the same with `"bynk.oidc"`, and a
`SignedCookie` actor reads the session cookie behind an `Origin` check, see below. The boundary admits
`None`, `Bearer`, `Oidc` and `SignedCookie` and rejects `Signature`.) Only on success is the handler run with a
fresh, **owned** `connection: Connection[ServerFrame]`.

### Authenticate a browser with a session cookie

A page that already has a [`SignedCookie`](/reference/actors/#signed-cookies) session can open the socket
without handling a token: the browser sends the cookie on the upgrade. Because a cookie is sent from
any site, and a WebSocket is not covered by CORS, the service says which pages may open it:

```bynk,fragment
actor Member { auth = SignedCookie(secret = "SESSION_KEY", ttl = 12.hours), identity = UserId }

service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
	cors { origins: ["https://app.example.com"], credentials: true }

	on open (roomId: RoomId) -> Effect[()] by user: Member {
		let _ <- Room(roomId).join(user.identity, connection)
	}
}
```

The upgrade is refused with `403` unless its `Origin` header is exactly one of those origins, and a missing
`Origin` is refused too, before the cookie is read. Without the `cors` block the service does not compile
(`bynk.ws.cookie_needs_origins`). The rules are in the
[reference](/reference/websocket/#authenticate-with-a-session-cookie).

That connection is a [held resource](/reference/types/#connection): it has
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

A held connection is never dropped with its socket left open. `put` over a key that
already holds a connection **closes the displaced connection first**, as `remove`
does, so a member who reconnects under the same key is not left holding a stale
socket. Putting back the connection the key already holds closes nothing. This
behaves the same in `bynk test` as when deployed, so a case can check it: after the
second `put`, the displaced `TestConnection` reports `closed()`.

A connection held under two keys is one socket, so closing it through either key, by
`remove` or by replacement, closes it for both. The other key keeps its entry, but the
connection behind it is closed: in `bynk test` it is the closed `TestConnection`, and
when deployed a closed socket no longer resolves, so `get` on that key returns `None`.

## Receive inbound frames and broadcast

Inbound frames arrive through `on message`; `on close` fires when the client
disconnects, and every WebSocket service needs one: `on open` handed the connection to the
room agent, and nothing else removes it when the client leaves (a service without `on close` is
`bynk.ws.missing_close`). Each delegates to the room agent, which broadcasts over its held map:

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

### Binary frames and frame limits

A frame is JSON text by default. For audio, images or a compact protocol, make the frame type `Bytes`:
`in: Bytes` delivers each inbound binary message to `on message` as `Bytes`, and `out: Bytes` sends
binary messages. A service declares the largest frame it reads or sends with `limits { maxFrame }`, in
bytes (1 MiB if you say nothing, at most 32 MiB):

```bynk,fragment
service Audio from websocket(in: Bytes, out: Bytes) {
  limits { maxFrame: 262_144 }
  on message (roomId: RoomId, frame: Bytes) -> Effect[()] by user: Participant {
    let _ <- connection.send(frame)
    ()
  }
}
```

A frame over `maxFrame` closes the connection with `1009` before it is parsed, a text frame to a `Bytes`
service and a binary frame to a JSON one with `1003`, and a frame your code sends over `maxFrame`
faults the send. The [reference](/reference/websocket/#frame-size-and-binary-frames) has the rules.

### Leaving a room

A client that goes away must be removed from the room, or a later broadcast raises on its dead
connection. `on close` is where that happens. To see why the connection ended, add a trailing
`close: CloseInfo` parameter (`consumes bynk { CloseInfo }`): its `code` and `reason` are the
client's close frame (`1000` is a normal closure, `1006` an abnormal disconnection with no frame),
and `clean` is whether the close handshake completed:

```bynk,ignore
on close (roomId: RoomId, close: CloseInfo) -> Effect[()] by user: Participant {
  let _ <- Room(roomId).leave(user.identity, close.code)
  ()
}
```

A connection that ends with a non-disconnection error runs `on close` once with
`CloseInfo { code: 1011, reason: "error", clean: false }`, so the room releases it the same way.

To close a connection from the server with a code the client can read, call
`connection.close(4001, "room archived")` in the agent. The code is `1000` or `3000` to `4999`
and the reason at most 123 bytes; a literal outside that is a compile error
(`bynk.ws.bad_close_code`, `bynk.ws.bad_close_reason`).

```bynk,fragment
  on call post(sender: UserId, text: String) -> Effect[()] {
    let _ <- conns.parForEach((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))
    ()
  }
```

`parForEach` sends to every connection **in parallel** (so one slow client cannot
stall the room, and it waits for every send to settle before it raises a fault); `forEach` is the sequential sibling. In the broadcast closure each
`c` is **borrowed** — you may `send` to it, but closing or transferring it is
`bynk.held.consume_on_borrow`. To broadcast to everyone *but* the sender, filter on
the sender's key (`u != sender`); a `Connection` cannot be compared with `==`
(`bynk.types.held_not_comparable`).

## Test it with no Cloudflare runtime

A `suite` drives the service by address, against a `TestConnection[F]()` that
records every frame the server sends (`.sent()`) and whether it closed the
connection (`.closed()`). The whole flow runs under Node:

```bynk,fragment
suite chat {
	case "a message reaches the room, and a member who left hears nothing" {
		let a = TestConnection[ServerFrame]()
		let b = TestConnection[ServerFrame]()
		let room = RoomId.unsafe("r1")
		let _ <- ChatGateway.open(a, room) by Participant(UserId.unsafe("alice"))
		let _ <- ChatGateway.open(b, room) by Participant(UserId.unsafe("bob"))
		expect a.sent() == [ServerFrame { text: "welcome" }]
		let _ <- ChatGateway.message(a, ClientFrame { text: "hello room" })
		// both a.sent() and b.sent() now end with "hello room"
		let _ <- ChatGateway.close(b)
		let _ <- ChatGateway.message(a, ClientFrame { text: "after leave" })
		expect b.sent().length() == 2  // bob has left
	}
}
```

`open` takes the route params and the actor (`by`); `message` and `close` reuse
both from the connection, as the platform does. To test an agent without the
service, pass it `tc.connection()`. See [Driving a WebSocket
service](/reference/testing/#websocket-address).

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

type ServerFrame = {
	text: String,
}

type ClientFrame = {
	text: String,
}

actor Participant { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

service ChatGateway from websocket(in: ClientFrame, out: ServerFrame) {
	on open(roomId: RoomId) -> Effect[()] by user: Participant {
		let _ <- connection.send(ServerFrame { text: "welcome" })
		let _ <- Room(roomId).join(user.identity, connection)
	}

	on message(roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant {
		let _ <- Room(roomId).post(user.identity, frame.text)
	}

	on close(roomId: RoomId) -> Effect[()] by user: Participant {
		let _ <- Room(roomId).leave(user.identity)
	}
}

agent Room {
	key id: RoomId

	store members: Set[UserId]
	store conns: Map[UserId, Connection[ServerFrame]]

	on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] {
		let _ <- members.add(u)
		let _ <- conns.put(u, conn)
	}

	on call leave(u: UserId) -> Effect[()] {
		let _ <- members.remove(u)
		let _ <- conns.remove(u)
	}

	on call post(sender: UserId, text: String) -> Effect[()] {
		let _ <- conns.parForEach((c: Connection[ServerFrame]) => c.send(
			ServerFrame { text: text }
		))
	}
}
```

## Related

- Reference: [WebSocket](/reference/websocket/) — the full handler surface,
  broadcast, authentication, and diagnostics.
- Reference: [Type system — Connection](/reference/types/#connection) — the
  held-resource type and its linearity discipline.
- Guide: [Handle an HTTP request](/book/guides/entry-points/http/) — the sibling request/response protocol.

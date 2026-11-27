// #3901: the client side of a `system` case's WebSocket address. A case's `svc.open(tc, …)` performs a real
// upgrade through the participant Worker's own `fetch`: the upgrade request a browser would send (the route
// values in the query string, the credential as the `Sec-WebSocket-Protocol` pair), answered with the `101` and
// the client end of the pair, or refused with the status the Worker gave. `svc.message` and `svc.close` then write on that end, and the server's frames and its close come back
// the same way, into the case's `TestConnection`, which `tc.sent()` and the rest of its reads see.
//
// The doubles the Worker's upgrade code needs (`WebSocketPair`, a `101` `Response`, a Durable Object that
// accepts sockets) are installed once, when this module loads: a module with this fragment has a WebSocket
// service to drive.
installTestWebSocketRuntime();

type __WsHandle = { socket: TestWebSocket | undefined; refused: number | undefined; ours: boolean };
const __wsHandles = new globalThis.WeakMap<object, __WsHandle>();

type __WsUpgrade = {
  service: string;
  // The route values by the handler's parameter names, in the order it declares them.
  params: readonly (readonly [string, unknown])[];
  // `Sec-WebSocket-Protocol` for the upgrade: `bynk.bearer, <token>`, or none.
  protocols: string | undefined;
};

function __wsHandleOf(tc: object, what: string): __WsHandle {
  const handle = __wsHandles.get(tc);
  if (handle === undefined) throw new globalThis.Error(`TestConnection: ${what} before open — call open of the service first`);
  if (handle.refused !== undefined) {
    throw new globalThis.Error(`TestConnection: ${what} on a connection whose upgrade the Worker refused with ${handle.refused}; read refusedWith() instead`);
  }
  return handle;
}

// What a handler in the Durable Object threw while a frame or a close ran: the platform logs it and keeps the
// socket, a case must see it.
function __wsFaults(service: string): void {
  const faults = takeWebSocketFaults();
  if (faults.length > 0) {
    const first = faults[0];
    throw new globalThis.Error(`the WebSocket service ${service} faulted: ${first instanceof globalThis.Error ? first.message : globalThis.String(first)}`);
  }
}

async function __bynkWsOpen<F>(
  tc: TestConnection<F>,
  fetchUpgrade: (request: globalThis.Request) => Promise<globalThis.Response>,
  upgrade: __WsUpgrade,
  // The out type's deserialiser takes the wire JSON; never lets any such function in.
  decode: (wire: never) => { readonly tag: "Ok"; readonly value: F } | { readonly tag: "Err"; readonly error: unknown },
): Promise<void> {
  // One socket has one owner, as at `unit`: a connection that was opened or handed to an agent is not opened again.
  tc.remember(upgrade.params.map(([, v]) => v), undefined);
  const handle: __WsHandle = { socket: undefined, refused: undefined, ours: false };
  __wsHandles.set(tc, handle);
  const url = new globalThis.URL("https://bynk.test/");
  for (const [name, value] of upgrade.params) url.searchParams.set(name, globalThis.String(value));
  const headers = new globalThis.Headers({ Upgrade: "websocket" });
  if (upgrade.protocols !== undefined) headers.set("Sec-WebSocket-Protocol", upgrade.protocols);
  const response = await fetchUpgrade(new globalThis.Request(url, { headers }));
  if (response.status !== 101) {
    handle.refused = response.status;
    tc.refused = response.status;
    return;
  }
  const socket = (response as unknown as { webSocket?: TestWebSocket }).webSocket;
  if (socket === undefined) throw new globalThis.Error(`the Worker answered the upgrade of ${upgrade.service} with 101 but no WebSocket`);
  handle.socket = socket;
  socket.addEventListener("message", (event) => {
    const data = (event as { data: string | ArrayBuffer }).data;
    const wire = typeof data === "string" ? (globalThis.JSON.parse(data) as unknown) : new globalThis.Uint8Array(data);
    const decoded = decode(wire as never);
    if (decoded.tag === "Err") throw new globalThis.Error(`${upgrade.service} sent a frame its own out type refuses`);
    tc.sent.push(decoded.value);
  });
  socket.addEventListener("close", (event) => {
    // The client going away (`svc.close(tc)`) is not the server closing, as at `unit`.
    if (handle.ours) return;
    const closed = event as { code: number; reason: string };
    tc.closed = true;
    tc.closeCode = closed.code;
    tc.closeReason = closed.reason === "" ? undefined : closed.reason;
  });
  await webSocketQuiet();
  __wsFaults(upgrade.service);
}

async function __bynkWsMessage<F>(tc: TestConnection<F>, service: string, wire: unknown): Promise<void> {
  const handle = __wsHandleOf(tc, "message");
  const socket = handle.socket as TestWebSocket;
  socket.send(wire instanceof globalThis.Uint8Array ? wire : globalThis.JSON.stringify(wire));
  await webSocketQuiet();
  __wsFaults(service);
}

async function __bynkWsClose<F>(
  tc: TestConnection<F>,
  service: string,
  info: { readonly code: number; readonly reason: string; readonly clean: boolean },
): Promise<void> {
  const handle = __wsHandleOf(tc, "close");
  const socket = handle.socket as TestWebSocket;
  handle.ours = true;
  if (info.clean) socket.closeFrame(info.code, info.reason);
  else socket.terminate();
  await webSocketQuiet();
  __wsFaults(service);
}

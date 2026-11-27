// #3901: the in-process Workers WebSocket surface a `system` case drives a real upgrade through.
//
// A `system` suite runs the participant Worker's own `fetch` in process, against test doubles for its
// bindings. A WebSocket upgrade needs three things Node does not have: the `WebSocketPair` global, a
// Durable Object state that accepts sockets for hibernation (`acceptWebSocket`, `getWebSockets`,
// attachments) and dispatches their frames to `webSocketMessage` and `webSocketClose`, and a `101`
// response carrying the client end (Node's `Response` refuses every status outside 200 to 599). This
// module is those three, for tests only: it is imported by `testing/bindings` (a test build's module)
// and nothing else, and `installTestWebSocketRuntime()` is the only thing that touches a global.
//
// The behaviour is the platform's as the generated code relies on it:
//   - a frame sent on one end of a pair reaches the other, in order, asynchronously;
//   - a socket the Durable Object accepted with `acceptWebSocket` is not an event target: its frames and
//     its close reach the object's `webSocketMessage` and `webSocketClose`, with the socket as the first
//     argument and its attachment intact;
//   - a close is answered by the other end, as `web_socket_auto_reply_to_close` does: both ends end up
//     closed, and the end that did not start it is told with the code and reason;
//   - a close on an already closed socket is ignored;
//   - an attachment is a structured clone, so what a later call reads is what was written then.

import { makeTestState, type DurableObjectState } from "../storage.ts";

type Listener = (event: unknown) => void;

/** What a Durable Object that accepted a socket answers with: the platform's hibernation entry points. */
export interface WebSocketHost {
  webSocketMessage?(ws: TestWebSocket, message: string | ArrayBuffer): unknown;
  webSocketClose?(ws: TestWebSocket, code: number, reason: string, wasClean: boolean): unknown;
  webSocketError?(ws: TestWebSocket, error: unknown): unknown;
}

// Activity the doubles started and have not finished, so a case can wait until the server has been quiet.
let inflight = 0;
// What a host's entry point threw. The platform logs it and keeps the socket; a test must see it.
const faults: unknown[] = [];

/** The errors a Durable Object's WebSocket entry points threw since the last call, oldest first. */
export function takeWebSocketFaults(): unknown[] {
  return faults.splice(0);
}

/**
 * Resolves once every frame and close the doubles delivered has been handled, including the work a handler
 * awaited, and nothing new has started for a whole turn of the event loop. Bounded, so a handler that
 * answers every frame with a frame cannot hold a case forever; reaching the bound is an error naming the
 * cause, never a return that reads as quiet, since a case would then check a half-finished exchange.
 */
export async function webSocketQuiet(maxTurns: number = 1000): Promise<void> {
  for (let turn = 0; turn < maxTurns; turn += 1) {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    if (inflight === 0) {
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      if (inflight === 0) return;
    }
  }
  throw new Error(`webSocketQuiet: ${inflight} WebSocket deliveries still in flight after ${maxTurns} turns`);
}

function track<T>(work: () => T | Promise<T>): void {
  inflight += 1;
  void (async (): Promise<void> => {
    try {
      await work();
    } catch (e) {
      faults.push(e);
    } finally {
      inflight -= 1;
    }
  })();
}

function wireData(data: string | ArrayBuffer | ArrayBufferView): string | ArrayBuffer {
  if (typeof data === "string") return data;
  if (data instanceof ArrayBuffer) return data.slice(0);
  const bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  return bytes.slice().buffer;
}

// The close codes a server end may send: any of 1000 to 4999 but the three that are reserved for the
// platform to report (`1005`, `1006`, `1015`). A page's end is narrower, as in a browser.
function checkClose(side: "client" | "server", code: number | undefined, reason: string | undefined): void {
  if (code === undefined) return;
  if (side === "client") {
    if (code !== 1000 && !(code >= 3000 && code <= 4999)) {
      throw new Error(`InvalidAccessError: a page may close with 1000 or 3000 to 4999, not ${code}`);
    }
  } else if (code < 1000 || code > 4999 || code === 1005 || code === 1006 || code === 1015) {
    throw new TypeError(`the close code ${code} is reserved`);
  }
  if (reason !== undefined && new TextEncoder().encode(reason).byteLength > 123) {
    throw new Error("a close reason is at most 123 bytes");
  }
}

export class TestWebSocket {
  readonly side: "client" | "server";
  /** 1 once the pair is made; 2 while a close is being answered; 3 when closed. */
  readyState: 0 | 1 | 2 | 3 = 1;
  binaryType = "arraybuffer";
  peer!: TestWebSocket;
  /** The Durable Object that accepted this (server) end for hibernation, if one did. */
  host: (() => WebSocketHost | undefined) | undefined;
  private attachment: unknown = null;
  private readonly listeners = new Map<string, Set<Listener>>();
  /** What this end has been told, kept for a test to read: frames it received as events. */
  readonly received: (string | ArrayBuffer)[] = [];
  // Frames that arrived before any `message` listener, and whether the close event has been fired to one.
  private readonly held: (string | ArrayBuffer)[] = [];
  private closeTold = false;
  closeEvent: { code: number; reason: string; wasClean: boolean } | undefined;

  constructor(side: "client" | "server") {
    this.side = side;
  }

  /**
   * A non-hibernatable server end: its frames reach its own listeners. The double delivers to listeners from
   * the start, where the platform holds an end's frames until `accept()`; a case never listens before it.
   */
  accept(): void {}

  addEventListener(type: string, listener: Listener): void {
    const set = this.listeners.get(type) ?? new Set<Listener>();
    set.add(listener);
    this.listeners.set(type, set);
    // A client end the Worker's upgrade answered with is held by the case until it attaches: the frames the server
    // wrote while it ran `on open`, and a close it already made, wait for the first listener of their kind.
    if (type === "message" && this.held.length > 0) {
      const held = this.held.splice(0);
      inflight += 1;
      queueMicrotask(() => {
        try {
          for (const data of held) this.fire("message", { type: "message", data });
        } finally {
          inflight -= 1;
        }
      });
    }
    if (type === "close" && this.closeEvent !== undefined && !this.closeTold) {
      this.closeTold = true;
      const event = this.closeEvent;
      inflight += 1;
      queueMicrotask(() => {
        try {
          this.fire("close", { type: "close", ...event });
        } finally {
          inflight -= 1;
        }
      });
    }
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }

  serializeAttachment(value: unknown): void {
    this.attachment = structuredClone(value);
  }

  deserializeAttachment(): unknown {
    return this.attachment === null ? null : structuredClone(this.attachment);
  }

  send(data: string | ArrayBuffer | ArrayBufferView): void {
    if (this.readyState !== 1) throw new Error("the WebSocket is not open");
    const wire = wireData(data);
    const peer = this.peer;
    inflight += 1;
    queueMicrotask(() => {
      try {
        peer.deliver(wire);
      } finally {
        inflight -= 1;
      }
    });
  }

  close(code?: number, reason?: string): void {
    // A close on a socket already closing or closed is ignored, as the platform does.
    if (this.readyState !== 1) return;
    checkClose(this.side, code, reason);
    this.readyState = 2;
    const peer = this.peer;
    inflight += 1;
    queueMicrotask(() => {
      try {
        peer.receiveClose(code ?? 1005, reason ?? "", true, this);
      } finally {
        inflight -= 1;
      }
    });
  }

  /**
   * A browser closing its page or navigating away: a close frame whose code a script could not send itself
   * (`1001`, say), which `close` refuses of a page's end. A case driving `svc.close(tc, info)` stands for that
   * client going away, so it sends the code it names.
   */
  closeFrame(code: number, reason: string): void {
    if (this.readyState !== 1) return;
    this.readyState = 2;
    const peer = this.peer;
    inflight += 1;
    queueMicrotask(() => {
      try {
        peer.receiveClose(code, reason, true, this);
      } finally {
        inflight -= 1;
      }
    });
  }

  /** The network dropped: no close frame is exchanged, and both ends are told `1006`. Client-side only. */
  terminate(): void {
    if (this.readyState === 3) return;
    this.readyState = 3;
    const peer = this.peer;
    inflight += 1;
    queueMicrotask(() => {
      try {
        peer.receiveClose(1006, "", false, undefined);
        this.finish(1006, "", false);
      } finally {
        inflight -= 1;
      }
    });
  }

  private fire(type: string, event: unknown): void {
    // A listener is the case's own code; what it throws is kept with the host's faults, and is never allowed to
    // escape into the microtask that delivered the frame, where it would be an uncaught exception.
    for (const l of [...(this.listeners.get(type) ?? [])]) {
      try {
        l(event);
      } catch (e) {
        faults.push(e);
      }
    }
  }

  private deliver(data: string | ArrayBuffer): void {
    if (this.readyState === 3) return;
    const host = this.host?.();
    if (host !== undefined) {
      track(() => host.webSocketMessage?.(this, data));
      return;
    }
    this.received.push(data);
    if ((this.listeners.get("message")?.size ?? 0) === 0) {
      this.held.push(data);
      return;
    }
    this.fire("message", { type: "message", data });
  }

  private receiveClose(code: number, reason: string, wasClean: boolean, initiator: TestWebSocket | undefined): void {
    if (this.readyState === 3) return;
    this.readyState = 3;
    const host = this.host?.();
    if (host !== undefined) {
      track(() => host.webSocketClose?.(this, code, reason, wasClean));
    } else {
      this.finish(code, reason, wasClean);
    }
    // The answer: the end that asked is told once it is done.
    initiator?.finish(code, reason, wasClean);
  }

  private finish(code: number, reason: string, wasClean: boolean): void {
    this.readyState = 3;
    if (this.closeEvent !== undefined) return;
    this.closeEvent = { code, reason, wasClean };
    if ((this.listeners.get("close")?.size ?? 0) > 0) {
      this.closeTold = true;
      this.fire("close", { type: "close", code, reason, wasClean });
    }
  }
}

/** The `WebSocketPair` global: `pair[0]` is the client end, `pair[1]` the server end. */
export class TestWebSocketPair {
  readonly 0: TestWebSocket;
  readonly 1: TestWebSocket;
  constructor() {
    const client = new TestWebSocket("client");
    const server = new TestWebSocket("server");
    client.peer = server;
    server.peer = client;
    this[0] = client;
    this[1] = server;
  }
}

export interface HibernatableTestState extends DurableObjectState {
  acceptWebSocket(ws: TestWebSocket, tags?: string[]): void;
  getWebSockets(tag?: string): TestWebSocket[];
}

/**
 * A Durable Object state that accepts sockets for hibernation, as `makeTestState` does not. `host` yields the
 * object the state belongs to (it is built after its state), whose `webSocketMessage` and `webSocketClose`
 * the accepted sockets' frames and closes reach.
 */
export function makeHibernatableTestState(name: string, host: () => WebSocketHost | undefined): HibernatableTestState {
  const accepted: { ws: TestWebSocket; tags: string[] }[] = [];
  return Object.assign(makeTestState(name), {
    acceptWebSocket(ws: TestWebSocket, tags: string[] = []): void {
      ws.host = host;
      accepted.push({ ws, tags });
    },
    getWebSockets(tag?: string): TestWebSocket[] {
      return accepted
        .filter((a) => a.ws.readyState !== 3 && (tag === undefined || a.tags.includes(tag)))
        .map((a) => a.ws);
    },
  });
}

interface UpgradeInit extends ResponseInit {
  webSocket?: unknown;
}

// How many callers hold the patch, and what to put back when the last lets go.
let installs = 0;
let restoreGlobals: (() => void) | undefined;

/**
 * Makes the three globals the generated upgrade code reaches exist: `WebSocketPair`, and a `Response` that
 * accepts status `101` and carries `webSocket`. Each call returns its own release; the globals are put back
 * when the last of them has been called, so two participant Workers that install and release in their own
 * teardowns do not take the patch from each other. A release called twice counts once.
 * A test build calls it when it stands up a participant Worker with a WebSocket service, nothing else does.
 */
export function installTestWebSocketRuntime(): () => void {
  if (installs === 0) restoreGlobals = patchGlobals();
  installs += 1;
  let released = false;
  return (): void => {
    if (released) return;
    released = true;
    installs -= 1;
    if (installs === 0) {
      restoreGlobals?.();
      restoreGlobals = undefined;
    }
  };
}

function patchGlobals(): () => void {
  const g = globalThis as unknown as { WebSocketPair?: unknown; Response: typeof Response };
  const before = { pair: g.WebSocketPair, response: g.Response };
  const Base = before.response;
  class UpgradeResponse extends Base {
    readonly webSocket: unknown;
    constructor(body?: BodyInit | null, init?: UpgradeInit) {
      const upgrade = init?.status === 101;
      super(upgrade ? null : body, upgrade ? { ...init, status: 200 } : init);
      if (upgrade) {
        Object.defineProperty(this, "status", { value: 101 });
        Object.defineProperty(this, "ok", { value: false });
      }
      this.webSocket = init?.webSocket;
    }
    // A response made before this was installed is still a `Response`.
    static [Symbol.hasInstance](value: unknown): boolean {
      return value instanceof Base;
    }
  }
  g.WebSocketPair = TestWebSocketPair;
  g.Response = UpgradeResponse as unknown as typeof Response;
  return (): void => {
    if (before.pair === undefined) delete g.WebSocketPair;
    else g.WebSocketPair = before.pair;
    g.Response = before.response;
  };
}

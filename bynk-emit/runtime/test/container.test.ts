import { test } from "node:test";
import assert from "node:assert/strict";
import {
  CONTAINER_ERROR_HEADER,
  CONTAINER_FAULT_HEADER,
  CONTAINERS_UNAVAILABLE_VAR,
  containerClass,
  resumeContainer,
  type ContainerHandle,
  type ContainerSettings,
} from "../src/container.ts";

// #3477: the generated class against a fake of `ctx.container`. Whether the
// platform behaves like the fake is for a real deploy to show.

type Call = { name: string; arg?: unknown };

function fakeContainer(over: Partial<{
  running: boolean;
  images: Record<string, string>;
  startError: Error;
  failFetches: number;
  answer: (url: string, init: any) => Response;
}> = {}): { handle: ContainerHandle; calls: Call[] } {
  const calls: Call[] = [];
  let running = over.running ?? false;
  let failures = over.failFetches ?? 0;
  const handle: ContainerHandle = {
    get running() {
      return running;
    },
    images: over.images ?? { Renderer: "registry.cloudflare.com/acct/renderer@sha256:abc" },
    start(options) {
      calls.push({ name: "start", arg: options });
      if (over.startError) throw over.startError;
      running = true;
    },
    async setInactivityTimeout(ms) {
      calls.push({ name: "setInactivityTimeout", arg: ms });
    },
    getTcpPort(port) {
      return {
        async fetch(url, init) {
          calls.push({ name: "fetch", arg: { port, url, init } });
          if (failures > 0) {
            failures -= 1;
            throw new Error("connection refused");
          }
          return (over.answer ?? (() => new Response("ok", { status: 200 })))(url, init);
        },
      };
    },
  };
  return { handle, calls };
}

const settings = (over: Partial<ContainerSettings> = {}): ContainerSettings => ({
  image: "Renderer",
  port: 8080,
  forward: [],
  ...over,
});

const make = (s: ContainerSettings, container: ContainerHandle, env: unknown = {}) =>
  new (containerClass(s))({ container }, env);

test("a first request starts the container with egress off, then proxies to the port", async () => {
  const { handle, calls } = fakeContainer();
  const res = await make(settings(), handle).fetch(
    new Request("http://renderer/render?x=1", { method: "POST", body: "hello" }),
  );
  assert.equal(res.status, 200);
  assert.equal(await res.text(), "ok");
  const start = calls.find((c) => c.name === "start")!.arg as Record<string, unknown>;
  assert.deepEqual(start, { image: "registry.cloudflare.com/acct/renderer@sha256:abc", enableInternet: false });
  const sent = calls.find((c) => c.name === "fetch")!.arg as any;
  assert.equal(sent.port, 8080);
  assert.equal(sent.url, "http://renderer/render?x=1");
  assert.equal(sent.init.method, "POST");
  assert.equal(new TextDecoder().decode(sent.init.body), "hello");
});

test("enableInternet is false however the container is configured", async () => {
  const { handle, calls } = fakeContainer();
  await make(settings({ instanceType: "standard-2", forward: ["KEY"], sleepAfterSeconds: 60 }), handle, { KEY: "v" }).fetch(
    new Request("http://r/"),
  );
  assert.equal((calls.find((c) => c.name === "start")!.arg as any).enableInternet, false);
});

test("@instanceType is the `instance` of start, and @forward its env", async () => {
  const { handle, calls } = fakeContainer();
  await make(settings({ instanceType: "standard-1", forward: ["A", "B"] }), handle, { A: "1", B: "2", C: "3" }).fetch(
    new Request("http://r/"),
  );
  const start = calls.find((c) => c.name === "start")!.arg as any;
  assert.equal(start.instance, "standard-1");
  assert.deepEqual(start.env, { A: "1", B: "2" });
});

test("@sleepAfter is applied on every start, in milliseconds, after start", async () => {
  const { handle, calls } = fakeContainer();
  await make(settings({ sleepAfterSeconds: 300 }), handle).fetch(new Request("http://r/"));
  assert.deepEqual(calls.map((c) => c.name).slice(0, 2), ["start", "setInactivityTimeout"]);
  assert.equal(calls[1]!.arg, 300_000);
});

test("a running container is not started again", async () => {
  const { handle, calls } = fakeContainer({ running: true });
  await make(settings({ sleepAfterSeconds: 60 }), handle).fetch(new Request("http://r/"));
  assert.equal(calls.filter((c) => c.name === "start").length, 0);
});

test("a Durable Object constructed over a running container reapplies @sleepAfter", async () => {
  const { handle, calls } = fakeContainer({ running: true });
  resumeContainer({ container: handle }, settings({ sleepAfterSeconds: 120 }));
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual(calls, [{ name: "setInactivityTimeout", arg: 120_000 }]);
  const none = fakeContainer({ running: true });
  resumeContainer({ container: none.handle }, settings());
  assert.deepEqual(none.calls, []);
});

test("a port that is not listening yet is retried until it answers", async () => {
  const { handle, calls } = fakeContainer({ failFetches: 2 });
  const res = await make(settings(), handle).fetch(new Request("http://r/", { method: "POST", body: "b" }));
  assert.equal(res.status, 200);
  const bodies = calls.filter((c) => c.name === "fetch").map((c) => new TextDecoder().decode((c.arg as any).init.body));
  assert.deepEqual(bodies, ["b", "b", "b"]);
});

test("a failed call on a container that was already running is not retried", async () => {
  const { handle, calls } = fakeContainer({ running: true, failFetches: 1 });
  const res = await make(settings(), handle).fetch(new Request("http://r/"));
  assert.equal(res.headers.get(CONTAINER_ERROR_HEADER), "start-failed");
  assert.equal(calls.filter((c) => c.name === "fetch").length, 1);
});

test("a refusal to start is reported: capacity as unavailable, anything else as a failed start", async () => {
  const limit = await make(settings(), fakeContainer({ startError: new Error("instance limit reached") }).handle).fetch(
    new Request("http://r/"),
  );
  assert.equal(limit.headers.get(CONTAINER_ERROR_HEADER), "unavailable");
  const other = await make(settings(), fakeContainer({ startError: new Error("bad image") }).handle).fetch(
    new Request("http://r/"),
  );
  assert.equal(other.headers.get(CONTAINER_ERROR_HEADER), "start-failed");
  assert.equal(await other.text(), "bad image");
});

test("a forwarded secret the Worker lacks is a failed start that names it, not a value", async () => {
  const { handle, calls } = fakeContainer();
  const res = await make(settings({ forward: ["API_KEY"] }), handle, {}).fetch(new Request("http://r/"));
  assert.equal(res.headers.get(CONTAINER_ERROR_HEADER), "start-failed");
  assert.match(await res.text(), /API_KEY/);
  assert.equal(calls.length, 0);
});

test("no container configured, or an image missing from the map, is a failed start", async () => {
  const bare = await new (containerClass(settings()))({}, {}).fetch(new Request("http://r/"));
  assert.equal(bare.headers.get(CONTAINER_ERROR_HEADER), "start-failed");
  const missing = await make(settings(), fakeContainer({ images: {} }).handle).fetch(new Request("http://r/"));
  assert.equal(missing.headers.get(CONTAINER_ERROR_HEADER), "start-failed");
});

test("a container's own responses pass through, 5xx included, and cannot forge the error header", async () => {
  const { handle } = fakeContainer({
    answer: () => new Response("boom", { status: 503, headers: { [CONTAINER_ERROR_HEADER]: "timeout", "x-a": "1" } }),
  });
  const res = await make(settings(), handle).fetch(new Request("http://r/"));
  assert.equal(res.status, 503);
  assert.equal(res.headers.get(CONTAINER_ERROR_HEADER), null);
  assert.equal(res.headers.get("x-a"), "1");
  assert.equal(await res.text(), "boom");
});

test("under `bynk dev` without Docker the class faults naming Docker, and starts nothing", async () => {
  const { handle, calls } = fakeContainer();
  const res = await make(settings(), handle, { [CONTAINERS_UNAVAILABLE_VAR]: "docker" }).fetch(
    new Request("http://r/"),
  );
  assert.equal(res.status, 500);
  assert.equal(res.headers.get(CONTAINER_FAULT_HEADER), "docker");
  assert.equal(res.headers.get(CONTAINER_ERROR_HEADER), null);
  const text = await res.text();
  assert.match(text, /Docker/);
  assert.match(text, /`Renderer`/);
  assert.deepEqual(calls, []);
  // And without a `ctx.container` at all (what `wrangler dev --enable-containers=false` gives).
  const bare = await new (containerClass(settings()))({}, { [CONTAINERS_UNAVAILABLE_VAR]: "docker" }).fetch(
    new Request("http://r/"),
  );
  assert.equal(bare.headers.get(CONTAINER_FAULT_HEADER), "docker");
});

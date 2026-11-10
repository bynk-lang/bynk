// Hand-written TypeScript twin of `examples/rate-limiter`.
//
// A fixed-window limiter: one Durable Object per client, keyed by name, holding
// the window start and the count in one storage value. The Worker reads the
// clock and passes it in, as the Bynk version does, so the policy stays a pure
// function of its inputs. Written with no libraries; see `../README.md`.

import { DurableObject } from "cloudflare:workers";

interface Env {
  LIMITER: DurableObjectNamespace<Limiter>;
}

interface Window {
  windowStart: number;
  count: number;
}

interface RateView {
  allowed: boolean;
  remaining: number;
  resetAt: number;
}

export class Limiter extends DurableObject {
  async hit(now: number, windowMs: number, limit: number): Promise<RateView> {
    const prev = (await this.ctx.storage.get<Window>("state")) ?? { windowStart: 0, count: 0 };
    const fresh = now - prev.windowStart >= windowMs;
    const windowStart = fresh ? now : prev.windowStart;
    const used = fresh ? 0 : prev.count;
    const allowed = used < limit;
    const count = allowed ? used + 1 : used;
    await this.ctx.storage.put<Window>("state", { windowStart, count });
    return { allowed, remaining: limit - count, resetAt: windowStart + windowMs };
  }
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { pathname } = new URL(request.url);
    const match = request.method === "GET" ? /^\/check\/([^/]+)$/.exec(pathname) : null;
    if (match === null) return new Response("Not Found", { status: 404 });
    let client: string;
    try {
      client = decodeURIComponent(match[1]);
    } catch {
      return json({ error: "client must be non-empty" }, 400);
    }
    if (client.length === 0) return json({ error: "client must be non-empty" }, 400);

    const stub = env.LIMITER.get(env.LIMITER.idFromName(client));
    const view = await stub.hit(Date.now(), 60000, 10);
    if (view.allowed) return json(view);
    return json({ error: `rate limit exceeded; window resets at ${view.resetAt}` }, 429);
  },
};

// Hand-written TypeScript twin of `examples/orders`.
//
// One Durable Object, `Book`, holds the orders and their line items. It is
// written the way a hand-written Durable Object normally is: one storage key per
// record (`o:<id>`, `l:<id>`), plus a small index key per order (`i:<orderId>`)
// listing its line ids, which is what the Bynk `@indexed(by: orderId)` gives the
// equality filter. Request bodies are validated by hand, with the same rules as
// the Bynk types (`id`, `customer`, `sku` are strings; `qty` is a positive safe
// integer). Written with no libraries; see `../README.md`.

import { DurableObject } from "cloudflare:workers";

interface Env {
  BOOK: DurableObjectNamespace<Book>;
}

interface Order {
  id: string;
  customer: string;
}

interface Line {
  id: string;
  orderId: string;
  sku: string;
  qty: number;
}

interface Sale {
  customer: string;
  sku: string;
  qty: number;
}

interface OrderTotal {
  orderId: string;
  units: number;
}

export class Book extends DurableObject {
  async addOrder(order: Order): Promise<Order> {
    await this.ctx.storage.put(`o:${order.id}`, order);
    return order;
  }

  async addLine(line: Line): Promise<void> {
    const indexKey = `i:${line.orderId}`;
    const ids = (await this.ctx.storage.get<string[]>(indexKey)) ?? [];
    if (!ids.includes(line.id)) ids.push(line.id);
    await this.ctx.storage.put({ [`l:${line.id}`]: line, [indexKey]: ids });
  }

  async linesFor(orderId: string): Promise<Line[]> {
    const ids = (await this.ctx.storage.get<string[]>(`i:${orderId}`)) ?? [];
    if (ids.length === 0) return [];
    const found = await this.ctx.storage.get<Line>(ids.map((id) => `l:${id}`));
    return ids.map((id) => found.get(`l:${id}`)).filter((l): l is Line => l !== undefined);
  }

  async sales(): Promise<Sale[]> {
    const orders = await this.ctx.storage.list<Order>({ prefix: "o:" });
    const lines = await this.ctx.storage.list<Line>({ prefix: "l:" });
    const byId = new Map<string, Order>();
    for (const o of orders.values()) byId.set(o.id, o);
    const out: Sale[] = [];
    for (const l of lines.values()) {
      const o = byId.get(l.orderId);
      if (o !== undefined) out.push({ customer: o.customer, sku: l.sku, qty: l.qty });
    }
    return out;
  }

  async totals(): Promise<OrderTotal[]> {
    const lines = await this.ctx.storage.list<Line>({ prefix: "l:" });
    const units = new Map<string, number>();
    for (const l of lines.values()) units.set(l.orderId, (units.get(l.orderId) ?? 0) + l.qty);
    return [...units].map(([orderId, u]) => ({ orderId, units: u }));
  }
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const bad = (message: string): Response => json({ error: message }, 400);

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function parseOrder(v: unknown): Order | string {
  if (!isRecord(v)) return "expected an object";
  if (typeof v.id !== "string") return "id must be a string";
  if (typeof v.customer !== "string") return "customer must be a string";
  return { id: v.id, customer: v.customer };
}

function parseLine(v: unknown): Line | string {
  if (!isRecord(v)) return "expected an object";
  if (typeof v.id !== "string") return "id must be a string";
  if (typeof v.orderId !== "string") return "orderId must be a string";
  if (typeof v.sku !== "string") return "sku must be a string";
  if (typeof v.qty !== "number" || !Number.isSafeInteger(v.qty)) return "qty must be an integer";
  if (v.qty <= 0) return "qty must be positive";
  return { id: v.id, orderId: v.orderId, sku: v.sku, qty: v.qty };
}

async function readJson(request: Request): Promise<unknown> {
  try {
    return await request.json();
  } catch {
    return undefined;
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { pathname } = new URL(request.url);
    const book = env.BOOK.get(env.BOOK.idFromName("default"));

    if (request.method === "POST" && pathname === "/orders") {
      const order = parseOrder(await readJson(request));
      if (typeof order === "string") return bad(order);
      return json(await book.addOrder(order), 201);
    }
    if (request.method === "POST" && pathname === "/lines") {
      const line = parseLine(await readJson(request));
      if (typeof line === "string") return bad(line);
      await book.addLine(line);
      return json(line, 201);
    }
    if (request.method === "GET") {
      const lines = /^\/orders\/([^/]+)\/lines$/.exec(pathname);
      if (lines !== null) {
        let id: string;
        try {
          id = decodeURIComponent(lines[1]);
        } catch {
          return bad("bad order id");
        }
        return json(await book.linesFor(id));
      }
      if (pathname === "/sales") return json(await book.sales());
      if (pathname === "/totals") return json(await book.totals());
    }
    return new Response("Not Found", { status: 404 });
  },
};

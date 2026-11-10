// Hand-written TypeScript twin of `examples/hello-world`.
//
// Written the way a careful TypeScript author would write a small Worker with
// no libraries: route by hand, validate the one input by hand, log with
// `console`. It matches the Bynk service's observable behaviour (status
// codes, JSON bodies, the 40-character limit); see `../README.md` for the
// fairness rules and `../bench.mjs` for the conformance check.

const MAX_SUBJECT = 40;

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function greeting(subject: string): string {
  return `Hello, ${subject}!`;
}

export default {
  async fetch(request: Request): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (request.method !== "GET") {
      return new Response("Not Found", { status: 404 });
    }
    if (pathname === "/") {
      console.info("greeting the world");
      return json(greeting("World"));
    }
    const match = /^\/hello\/([^/]+)$/.exec(pathname);
    if (match !== null) {
      let name: string;
      try {
        name = decodeURIComponent(match[1]);
      } catch {
        return json({ error: "a name must be non-empty and at most 40 characters" }, 400);
      }
      if (name.length === 0 || name.length > MAX_SUBJECT) {
        return json({ error: "a name must be non-empty and at most 40 characters" }, 400);
      }
      console.info(`greeting ${name}`);
      return json(greeting(name));
    }
    return new Response("Not Found", { status: 404 });
  },
};

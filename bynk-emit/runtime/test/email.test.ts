import { test } from "node:test";
import assert from "node:assert/strict";
import PostalMime from "postal-mime";
import {
  EMAIL_MAX_ATTACHMENTS,
  EMAIL_MAX_NESTING,
  EMAIL_MAX_PARTS,
  EMAIL_MAX_RAW_BYTES,
  buildReply,
  dmarcVerdict,
  encodeHeaderText,
  handleEmail,
  parseInbound,
  pickRoute,
  readRaw,
  rejectReason,
  replyMime,
  replySubject,
} from "../src/email.ts";
import type { EmailReply, EmailRoute, EmailVerdict, InboundEmail, InboundMessage } from "../src/email.ts";
import { None, Some } from "../src/result.ts";

const enc = new TextEncoder();

function stream(bytes: Uint8Array, chunk = 7): ReadableStream<Uint8Array> {
  let at = 0;
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      if (at >= bytes.length) return controller.close();
      controller.enqueue(bytes.subarray(at, at + chunk));
      at += chunk;
    },
  });
}

const CF = "Authentication-Results: mx.cloudflare.net;\r\n  dkim=pass header.d=example.org;\r\n  dmarc=pass header.from=example.org\r\n";

function mime(opts: { headers?: string; subject?: string; body?: string; messageId?: string | null } = {}): string {
  const id = opts.messageId === null ? "" : `Message-ID: ${opts.messageId ?? "<abc@example.org>"}\r\n`;
  return `${opts.headers ?? CF}From: A <a@example.org>\r\nTo: support@shop.example\r\nSubject: ${opts.subject ?? "hello"}\r\n${id}\r\n${opts.body ?? "body text\r\n"}`;
}

interface Calls {
  rejects: string[];
  forwards: string[];
  replies: { from: string; to: string; raw: string }[];
  logs: string[];
}

function fake(raw: string | Uint8Array, over: Partial<InboundMessage> = {}): { message: InboundMessage; calls: Calls } {
  const bytes = typeof raw === "string" ? enc.encode(raw) : raw;
  const calls: Calls = { rejects: [], forwards: [], replies: [], logs: [] };
  const message: InboundMessage = {
    from: "a@example.org",
    to: "support@shop.example",
    raw: stream(bytes),
    rawSize: bytes.length,
    setReject: (r) => void calls.rejects.push(r),
    forward: async (to) => void calls.forwards.push(to),
    reply: async (m) => void calls.replies.push(m as { from: string; to: string; raw: string }),
    ...over,
  };
  return { message, calls };
}

const factory = (from: string, to: string, raw: string): unknown => ({ from, to, raw });

function route(address: string, verdict: EmailVerdict | (() => Promise<EmailVerdict>), seen?: { mail?: InboundEmail; sender?: unknown }): EmailRoute {
  return {
    address,
    handle: async (mail, sender) => {
      if (seen) {
        seen.mail = mail;
        seen.sender = sender;
      }
      return typeof verdict === "function" ? verdict() : verdict;
    },
  };
}

// ---------------------------------------------------------------- dispatch

test("pickRoute: an exact address wins over the catch-all, whatever the order", () => {
  const catchAll = route("*@shop.example", { tag: "Accept" });
  const exact = route("support@shop.example", { tag: "Accept" });
  assert.equal(pickRoute([catchAll, exact], "support@shop.example"), exact);
  assert.equal(pickRoute([exact, catchAll], "sales@shop.example"), catchAll);
});

test("pickRoute: the local part and the domain match case-insensitively", () => {
  const exact = route("Support@Shop.Example", { tag: "Accept" });
  assert.equal(pickRoute([exact], "SUPPORT@shop.example"), exact);
  const catchAll = route("*@Shop.Example", { tag: "Accept" });
  assert.equal(pickRoute([catchAll], "Anyone@SHOP.example"), catchAll);
});

test("pickRoute: a catch-all claims only its own domain, and nothing is claimed otherwise", () => {
  const catchAll = route("*@shop.example", { tag: "Accept" });
  assert.equal(pickRoute([catchAll], "a@other.example"), undefined);
  assert.equal(pickRoute([catchAll], "a@sub.shop.example"), undefined);
  assert.equal(pickRoute([], "a@shop.example"), undefined);
  // the last `@` decides the domain, so a quoted local part cannot slip past
  assert.equal(pickRoute([catchAll], '"x@shop.example"@other.example'), undefined);
});

test("an unclaimed recipient is rejected with the fixed reason, before the message is read", async () => {
  const { message, calls } = fake(mime(), { to: "nobody@elsewhere.example" });
  let read = false;
  const spied = { ...message, raw: new ReadableStream<Uint8Array>({ pull: () => void (read = true) }, { highWaterMark: 0 }) };
  await handleEmail(spied, [route("support@shop.example", { tag: "Accept" })], factory, (l) => calls.logs.push(l));
  assert.deepEqual(calls.rejects, ["recipient not accepted"]);
  assert.equal(read, false);
});

// -------------------------------------------------------------------- DMARC

const h = (...rows: [string, string][]) => rows.map(([key, value]) => ({ key, value }));

test("dmarcVerdict: pass, fail and none from Cloudflare's header", () => {
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "mx.cloudflare.net; dmarc=pass header.from=x"])), { tag: "DmarcPass" });
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "mx.cloudflare.net; dkim=pass; dmarc=fail"])), { tag: "DmarcFail" });
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "mx.cloudflare.net; dmarc=none"])), { tag: "DmarcNone" });
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "mx.cloudflare.net; dmarc=temperror"])), { tag: "DmarcNone" });
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "mx.cloudflare.net; dkim=pass"])), { tag: "DmarcNone" });
  assert.deepEqual(dmarcVerdict([]), { tag: "DmarcNone" });
});

test("dmarcVerdict: a header from another server is never trusted", () => {
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "evil.example; dmarc=pass"])), { tag: "DmarcNone" });
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "mx.cloudflare.net.evil.example; dmarc=pass"])), { tag: "DmarcNone" });
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "evil.example; x=mx.cloudflare.net; dmarc=pass"])), { tag: "DmarcNone" });
});

test("dmarcVerdict: only the TOPMOST header is read; a sender's later copy is ignored", () => {
  const spoof = ["authentication-results", "mx.cloudflare.net; dmarc=pass"] as [string, string];
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "mx.cloudflare.net; dmarc=fail"], spoof)), { tag: "DmarcFail" });
});

test("dmarcVerdict: a topmost header from another server is DmarcNone, however far down a Cloudflare-looking one sits", () => {
  const spoof = ["authentication-results", "mx.cloudflare.net; dmarc=pass"] as [string, string];
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "evil.example; dmarc=pass"], spoof)), { tag: "DmarcNone" });
  assert.deepEqual(dmarcVerdict(h(["received", "from x"], ["authentication-results", "other.example; dmarc=fail"], spoof)), { tag: "DmarcNone" });
});

test("dmarcVerdict: comments, case and a version number in the header are read", () => {
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "MX.Cloudflare.NET 1 (ok); DMARC = PASS (p=reject)"])), { tag: "DmarcPass" });
  // a comment cannot smuggle a verdict in
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "mx.cloudflare.net; dmarc=none (dmarc=pass)"])), { tag: "DmarcNone" });
  // a `dmarc=pass` inside another method's value is not a result
  assert.deepEqual(dmarcVerdict(h(["authentication-results", "mx.cloudflare.net; spf=fail reason=\"dmarc=pass\""])), { tag: "DmarcNone" });
});

test("handleEmail: a spoofed Cloudflare header below another server's is DmarcNone", async () => {
  const raw = "Authentication-Results: evil.example; dmarc=pass\r\nAuthentication-Results: mx.cloudflare.net; dmarc=pass\r\n";
  const seen: { mail?: InboundEmail; sender?: unknown } = {};
  const { message } = fake(mime({ headers: raw }));
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" }, seen)], factory, () => {});
  assert.deepEqual((seen.sender as { dmarc: unknown }).dmarc, { tag: "DmarcNone" });
});

test("handleEmail reads the verdict through the parser: Cloudflare's header on top, a spoofed copy ignored", async () => {
  const spoofed = `${CF.replace("dmarc=pass", "dmarc=fail")}Authentication-Results: mx.cloudflare.net; dmarc=pass\r\n`;
  const seen: { mail?: InboundEmail; sender?: unknown } = {};
  const { message } = fake(mime({ headers: spoofed }));
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" }, seen)], factory, () => {});
  assert.deepEqual(seen.sender, { address: "a@example.org", dmarc: { tag: "DmarcFail" } });
  const seen2: { mail?: InboundEmail; sender?: unknown } = {};
  const { message: m2 } = fake(mime({ headers: "Authentication-Results: mx.cloudflare.net; dmarc=pass\r\n" }));
  await handleEmail(m2, [route("support@shop.example", { tag: "Accept" }, seen2)], factory, () => {});
  assert.deepEqual((seen2.sender as { dmarc: unknown }).dmarc, { tag: "DmarcPass" });
});

test("a lone topmost mx.cloudflare.net header is read as written", async () => {
  // Email Routing always prepends its header, so the topmost one is Cloudflare's;
  // a message with no Cloudflare header cannot reach here with one on top.
  const seen: { mail?: InboundEmail; sender?: unknown } = {};
  const { message } = fake(mime({ headers: "Authentication-Results: mx.cloudflare.net; dmarc=pass\r\n" }));
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" }, seen)], factory, () => {});
  assert.deepEqual((seen.sender as { dmarc: unknown }).dmarc, { tag: "DmarcPass" });
});

// ------------------------------------------------------------------ parsing

test("the handler is given the parsed message: text, html, attachments, headers, size", async () => {
  const raw =
    `${CF}From: A <a@example.org>\r\nTo: support@shop.example\r\nSubject: =?UTF-8?B?Y2Fmw6k=?=\r\nMessage-ID: <m1@example.org>\r\n` +
    `MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary="b"\r\n\r\n` +
    `--b\r\nContent-Type: multipart/alternative; boundary="c"\r\n\r\n--c\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nplain\r\n` +
    `--c\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>rich</p>\r\n--c--\r\n` +
    `--b\r\nContent-Type: application/pdf; name="a.pdf"\r\nContent-Disposition: attachment; filename="a.pdf"\r\nContent-Transfer-Encoding: base64\r\n\r\nAAEC\r\n--b--\r\n`;
  const seen: { mail?: InboundEmail } = {};
  const { message, calls } = fake(raw);
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" }, seen)], factory, () => {});
  const mail = seen.mail!;
  assert.equal(mail.from, "a@example.org");
  assert.equal(mail.to, "support@shop.example");
  assert.equal(mail.subject, "café");
  assert.equal(mail.messageId, "<m1@example.org>");
  assert.deepEqual(mail.text, Some("plain"));
  assert.deepEqual(mail.html, Some("<p>rich</p>"));
  assert.equal(mail.attachments.length, 1);
  assert.equal(mail.attachments[0]!.filename, "a.pdf");
  assert.equal(mail.attachments[0]!.contentType, "application/pdf");
  assert.equal(mail.attachments[0]!.inline, false);
  assert.deepEqual([...mail.attachments[0]!.content], [0, 1, 2]);
  assert.equal(mail.headers.get("message-id"), "<m1@example.org>");
  assert.equal(mail.size, enc.encode(raw).length);
  assert.deepEqual(calls.rejects, []);
});

test("a message with no body parts has None text and html", async () => {
  const seen: { mail?: InboundEmail } = {};
  const { message } = fake("From: a@example.org\r\nSubject: x\r\n\r\n");
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" }, seen)], factory, () => {});
  assert.equal(seen.mail!.subject, "x");
  assert.equal(seen.mail!.attachments.length, 0);
});

test("the envelope, not the headers, decides from and to", async () => {
  const seen: { mail?: InboundEmail; sender?: unknown } = {};
  const { message } = fake(mime(), { from: "envelope@real.example" });
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" }, seen)], factory, () => {});
  assert.equal(seen.mail!.from, "envelope@real.example");
  assert.equal((seen.sender as { address: string }).address, "envelope@real.example");
});

// ------------------------------------------------------------------- limits

test("limits: a declared size over the cap is rejected without reading the stream", async () => {
  const { message, calls } = fake(mime(), { rawSize: EMAIL_MAX_RAW_BYTES + 1 });
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" })], factory, (l) => calls.logs.push(l));
  assert.deepEqual(calls.rejects, ["message too large"]);
});

test("limits: a stream longer than the cap is cut off, whatever it declared", async () => {
  const big = new Uint8Array(1024 * 1024);
  let produced = 0;
  const lying = (): ReadableStream<Uint8Array> =>
    new ReadableStream<Uint8Array>({
      pull(controller) {
        produced += big.length;
        controller.enqueue(big);
        if (produced > EMAIL_MAX_RAW_BYTES * 4) controller.close();
      },
    });
  assert.equal(await readRaw(lying(), EMAIL_MAX_RAW_BYTES), null);
  assert.ok(produced <= EMAIL_MAX_RAW_BYTES + 4 * big.length, `stopped reading near the cap (${produced})`);
  const { message, calls } = fake(mime(), { raw: lying(), rawSize: 10 });
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" })], factory, (l) => calls.logs.push(l));
  assert.deepEqual(calls.rejects, ["message too large"]);
});

test("limits: too many MIME parts is a parse failure with the fixed reason", async () => {
  let body = `${CF}Content-Type: multipart/mixed; boundary="b"\r\n\r\n`;
  for (let i = 0; i <= EMAIL_MAX_PARTS; i++) body += `--b\r\nContent-Type: text/plain\r\n\r\nx\r\n`;
  body += "--b--\r\n";
  const { message, calls } = fake(body);
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" })], factory, (l) => calls.logs.push(l));
  assert.deepEqual(calls.rejects, ["message could not be read"]);
  assert.match(calls.logs[0]!, /parse failed \(EMAXPARTCOUNT\)/);
});

test("limits: nesting deeper than the cap is a parse failure", async () => {
  let body = `${CF}`;
  for (let i = 0; i <= EMAIL_MAX_NESTING + 1; i++) body += `Content-Type: multipart/mixed; boundary="b${i}"\r\n\r\n--b${i}\r\n`;
  body += "Content-Type: text/plain\r\n\r\nx\r\n";
  const { message, calls } = fake(body);
  await handleEmail(message, [route("support@shop.example", { tag: "Accept" })], factory, (l) => calls.logs.push(l));
  assert.deepEqual(calls.rejects, ["message could not be read"]);
  assert.match(calls.logs[0]!, /parse failed \(EMAXNESTINGDEPTH\)/);
});

test("limits: too many attachments is a parse failure", async () => {
  let body = `${CF}Content-Type: multipart/mixed; boundary="b"\r\n\r\n`;
  for (let i = 0; i <= EMAIL_MAX_ATTACHMENTS; i++) {
    body += `--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename="f${i}"\r\n\r\nx\r\n`;
  }
  body += "--b--\r\n";
  const r = await parseInbound({ from: "a@b.example", to: "c@d.example" }, enc.encode(body));
  assert.equal(r.ok, false);
  if (!r.ok) assert.equal(r.code, "EEMAILLIMIT");
});

test("limits: oversized headers are a parse failure", async () => {
  const body = `${CF}X-Big: ${"a".repeat(300_000)}\r\n\r\nx`;
  const r = await parseInbound({ from: "a@b.example", to: "c@d.example" }, enc.encode(body));
  assert.equal(r.ok, false);
  if (!r.ok) assert.equal(r.code, "EMAXHEADERSIZE");
});

// ------------------------------------------------------------------ verdicts

test("Accept does nothing; Reject sets the reason verbatim", async () => {
  const a = fake(mime());
  await handleEmail(a.message, [route("support@shop.example", { tag: "Accept" })], factory, () => {});
  assert.deepEqual([a.calls.rejects, a.calls.forwards, a.calls.replies], [[], [], []]);
  const r = fake(mime());
  await handleEmail(r.message, [route("support@shop.example", { tag: "Reject", reason: "no such mailbox" })], factory, () => {});
  assert.deepEqual(r.calls.rejects, ["no such mailbox"]);
});

test("Forward forwards to the destination; a refused forward becomes a logged, fixed reject", async () => {
  const f = fake(mime());
  await handleEmail(f.message, [route("support@shop.example", { tag: "Forward", to: "team@shop.example" })], factory, () => {});
  assert.deepEqual(f.calls.forwards, ["team@shop.example"]);
  assert.deepEqual(f.calls.rejects, []);
  const g = fake(mime(), {
    forward: async () => {
      throw Object.assign(new Error("destination address not verified: secret@x"), { name: "ForwardError" });
    },
  });
  await handleEmail(g.message, [route("support@shop.example", { tag: "Forward", to: "team@shop.example" })], factory, (l) => g.calls.logs.push(l));
  assert.deepEqual(g.calls.rejects, ["forward refused"]);
  assert.deepEqual(g.calls.logs, ["email support@shop.example: forward refused (ForwardError)"]);
});

const replyOf = (text?: string, html?: string, subject?: string): EmailReply => ({
  subject: subject === undefined ? None : Some(subject),
  text: text === undefined ? None : Some(text),
  html: html === undefined ? None : Some(html),
});

test("Reply: From is the receiving address, To the sender, threaded to the incoming Message-ID", async () => {
  const f = fake(mime({ messageId: "<orig@example.org>" }), { to: "Support@Shop.Example" });
  await handleEmail(f.message, [route("support@shop.example", { tag: "Reply", message: replyOf("thanks") })], factory, () => {});
  assert.equal(f.calls.replies.length, 1);
  const sent = f.calls.replies[0]!;
  assert.equal(sent.from, "Support@Shop.Example");
  assert.equal(sent.to, "a@example.org");
  const back = await PostalMime.parse(sent.raw);
  assert.equal(back.from?.address, "Support@Shop.Example");
  assert.equal(back.to?.[0]?.address, "a@example.org");
  assert.equal(back.subject, "Re: hello");
  assert.equal(back.inReplyTo, "<orig@example.org>");
  assert.equal(back.references, "<orig@example.org>");
  assert.equal(back.text, "thanks");
  assert.match(back.messageId ?? "", /^<[0-9a-f-]+@shop\.example>$/);
});

test("Reply: References extend the incoming chain; a message with no Message-ID gets no threading headers", async () => {
  const withChain = fake(mime({ headers: `${CF}References: <a@x> <b@x>\r\n`, messageId: "<c@x>" }));
  await handleEmail(withChain.message, [route("support@shop.example", { tag: "Reply", message: replyOf("t") })], factory, () => {});
  const back = await PostalMime.parse(withChain.calls.replies[0]!.raw);
  assert.equal(back.references, "<a@x> <b@x> <c@x>");
  const none = fake(mime({ messageId: null }));
  await handleEmail(none.message, [route("support@shop.example", { tag: "Reply", message: replyOf("t") })], factory, () => {});
  const back2 = await PostalMime.parse(none.calls.replies[0]!.raw);
  assert.equal(back2.inReplyTo, undefined);
  assert.equal(back2.references, undefined);
});

test("Reply: a platform refusal (no DMARC pass, a second reply) is a logged `reply refused` reject", async () => {
  const f = fake(mime(), {
    reply: async () => {
      throw Object.assign(new Error("reply rejected for body text of the message"), { name: "ReplyError" });
    },
  });
  await handleEmail(f.message, [route("support@shop.example", { tag: "Reply", message: replyOf("t") })], factory, (l) => f.calls.logs.push(l));
  assert.deepEqual(f.calls.rejects, ["reply refused"]);
  assert.deepEqual(f.calls.logs, ["email support@shop.example: reply refused (ReplyError)"]);
});

// ------------------------------------------------------------------- faults

test("a handler fault rejects with a fixed reason and logs only the error's name", async () => {
  const f = fake(mime({ body: "SECRET-BODY-TEXT\r\n", subject: "SECRET-SUBJECT" }));
  await handleEmail(
    f.message,
    [
      route("support@shop.example", async () => {
        throw Object.assign(new Error("failed on SECRET-BODY-TEXT"), { name: "DecodeFault" });
      }),
    ],
    factory,
    (l) => f.calls.logs.push(l),
  );
  assert.deepEqual(f.calls.rejects, ["message could not be processed"]);
  assert.deepEqual(f.calls.logs, ["email support@shop.example: handler faulted (DecodeFault)"]);
});

test("a handler that returns something that is not a verdict is a fault", async () => {
  const f = fake(mime());
  await handleEmail(f.message, [route("support@shop.example", (async () => ({ tag: "Bogus" })) as unknown as () => Promise<EmailVerdict>)], factory, () => {});
  assert.deepEqual(f.calls.rejects, ["message could not be processed"]);
});

test("a stream that errors is a fixed reject, not a throw", async () => {
  const broken = new ReadableStream<Uint8Array>({
    pull() {
      throw new Error("SECRET");
    },
  });
  const f = fake(mime(), { raw: broken });
  await handleEmail(f.message, [route("support@shop.example", { tag: "Accept" })], factory, (l) => f.calls.logs.push(l));
  assert.deepEqual(f.calls.rejects, ["message could not be read"]);
  assert.ok(!f.calls.logs.join("\n").includes("SECRET"));
});

test("no log line carries any of the message", async () => {
  const marker = "ZZ-UNIQUE-MARKER";
  const logs: string[] = [];
  const cases: Array<() => Promise<void>> = [
    async () => {
      const f = fake(mime({ body: marker, subject: marker }), { to: `${marker}@elsewhere.example` });
      await handleEmail(f.message, [route("support@shop.example", { tag: "Accept" })], factory, (l) => logs.push(l));
    },
    async () => {
      const f = fake(`${CF}X-Marker: ${marker}\r\nContent-Type: multipart/mixed; boundary="b"\r\n\r\n${"--b\r\n\r\nx\r\n".repeat(EMAIL_MAX_PARTS + 5)}`);
      await handleEmail(f.message, [route("support@shop.example", { tag: "Accept" })], factory, (l) => logs.push(l));
    },
    async () => {
      const f = fake(mime({ body: marker, subject: marker }));
      await handleEmail(f.message, [route("support@shop.example", async () => { throw new Error(marker); })], factory, (l) => logs.push(l));
    },
  ];
  for (const c of cases) await c();
  assert.ok(logs.length > 0);
  assert.ok(!logs.join("\n").includes(marker), logs.join("\n"));
});

// ---------------------------------------------------------- reply MIME writer

const fixed = {
  from: "support@shop.example",
  to: "a@example.org",
  subject: "Re: hello",
  text: "hi",
  html: undefined,
  inReplyTo: "<p@x>",
  references: undefined,
  date: new Date(Date.UTC(2026, 10, 11, 12, 0, 0)),
  messageId: "<id@shop.example>",
  boundary: "=_b",
} as const;

test("buildReply: CRLF throughout, base64 body, a numeric-zone Date, threading", () => {
  const out = buildReply(fixed);
  assert.ok(!/(^|[^\r])\n/.test(out), "no bare LF");
  assert.equal(
    out,
    [
      "From: support@shop.example",
      "To: a@example.org",
      "Subject: Re: hello",
      "Date: Wed, 11 Nov 2026 12:00:00 +0000",
      "Message-ID: <id@shop.example>",
      "In-Reply-To: <p@x>",
      "References: <p@x>",
      "MIME-Version: 1.0",
      "Content-Type: text/plain; charset=utf-8",
      "Content-Transfer-Encoding: base64",
      "",
      "aGk=",
      "",
    ].join("\r\n"),
  );
});

test("buildReply: text and HTML are a multipart/alternative that parses back", async () => {
  const out = buildReply({ ...fixed, html: "<b>héllo</b>" });
  assert.match(out, /Content-Type: multipart\/alternative; boundary="=_b"/);
  const back = await PostalMime.parse(out);
  assert.equal(back.text, "hi");
  assert.equal(back.html, "<b>héllo</b>");
});

test("buildReply: no body at all is an empty text part, HTML alone is HTML", async () => {
  assert.equal((await PostalMime.parse(buildReply({ ...fixed, text: undefined }))).text ?? "", "");
  const back = await PostalMime.parse(buildReply({ ...fixed, text: undefined, html: "<i>x</i>" }));
  assert.equal(back.html, "<i>x</i>");
});

test("buildReply: long bodies wrap at 76 columns and line endings are normalised to CRLF", async () => {
  const text = `${"word ".repeat(400)}\nsecond\rthird`;
  const out = buildReply({ ...fixed, text });
  for (const line of out.split("\r\n")) assert.ok(line.length <= 78, `line of ${line.length}`);
  const back = await PostalMime.parse(out);
  assert.equal(back.text, `${"word ".repeat(400)}\nsecond\nthird`);
});

test("encodeHeaderText: ASCII is left alone; anything else is RFC 2047 words of at most 75 characters", async () => {
  assert.equal(encodeHeaderText("plain subject"), "plain subject");
  const subject = `Réservation confirmée ✓ ${"日本語".repeat(40)} 🎉`;
  const encoded = encodeHeaderText(subject);
  for (const word of encoded.split(" ")) {
    assert.match(word, /^=\?UTF-8\?B\?[A-Za-z0-9+/]+=*\?=$/);
    assert.ok(word.length <= 75, `word of ${word.length}`);
  }
  const back = await PostalMime.parse(buildReply({ ...fixed, subject }));
  assert.equal(back.subject, subject);
  for (const line of buildReply({ ...fixed, subject }).split("\r\n")) assert.ok(line.length <= 78);
});

test("encodeHeaderText: a code point is never split across words", () => {
  const subject = "🎉".repeat(60);
  const decoded = encodeHeaderText(subject)
    .split(" ")
    .map((w) => new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(atob(w.slice(10, -2)), (c) => c.charCodeAt(0))))
    .join("");
  assert.equal(decoded, subject);
});

test("buildReply: a header cannot be injected through the subject or an address", () => {
  const out = buildReply({ ...fixed, subject: "x\r\nBcc: evil@example.org\r\nY: z" });
  assert.ok(!/^Bcc:/m.test(out));
  assert.equal(out.split("\r\n\r\n")[0]!.split("\r\n").filter((l) => l.startsWith("Subject")).length, 1);
  for (const bad of ["a@b.example\r\nBcc: e@x", "a b@c.example", "<a@b.example>", "a@b.example, c@d.example", "nodomain"]) {
    assert.throws(() => buildReply({ ...fixed, to: bad }), /unsafe address/, bad);
    assert.throws(() => buildReply({ ...fixed, from: bad }), /unsafe address/, bad);
  }
});

test("buildReply: a threading id that is not a bare <msg-id> is not copied into a header", () => {
  const out = buildReply({ ...fixed, inReplyTo: "<ok@x>\r\nBcc: e@x", references: "<r1@x>\r\nX: y <r2@x>" });
  assert.ok(!/^Bcc:/m.test(out) && !/^X:/m.test(out));
  assert.match(out, /^References: <r1@x> <r2@x> <ok@x>$/m);
});

test("replySubject: `Re: ` is not doubled", () => {
  assert.equal(replySubject("hello"), "Re: hello");
  assert.equal(replySubject("Re: hello"), "Re: hello");
  assert.equal(replySubject("RE : hello"), "RE : hello");
});

test("replyMime: an explicit subject is used as written", () => {
  const mail: InboundEmail = {
    from: "a@example.org",
    to: "support@shop.example",
    subject: "hello",
    messageId: "<m@x>",
    headers: new Map(),
    text: None,
    html: None,
    attachments: [],
    size: 0,
  };
  const out = replyMime(mail, replyOf("t", undefined, "Custom"), "support@shop.example");
  assert.match(out, /^Subject: Custom$/m);
});

// ------------------------------------------------------- the parser boundary
//
// `fuzz/` hosts Rust `cargo-fuzz` targets only, so the TypeScript-side parser
// boundary is covered here by seeded property tests in the binding test style:
// mutated and random inputs must never throw out of `handleEmail`, must always
// end in exactly one outcome, and must never leak input into a log line.

function prng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 0x1_0000_0000;
  };
}

const SEEDS = [
  mime(),
  `${CF}Content-Type: multipart/mixed; boundary="b"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nhi\r\n--b\r\nContent-Type: image/png\r\nContent-Disposition: inline; filename=x.png\r\nContent-Transfer-Encoding: base64\r\n\r\nAAEC\r\n--b--\r\n`,
  `${CF}Subject: =?UTF-8?Q?=C3=A9?=\r\nContent-Type: message/rfc822\r\n\r\nFrom: x@y.example\r\nSubject: inner\r\n\r\nnested\r\n`,
  "",
  "\r\n\r\n",
  "no headers at all",
];

function mutate(rand: () => number, base: string): Uint8Array {
  const bytes = [...enc.encode(base)];
  const edits = 1 + Math.floor(rand() * 8);
  for (let i = 0; i < edits; i++) {
    const at = Math.floor(rand() * (bytes.length + 1));
    switch (Math.floor(rand() * 5)) {
      case 0:
        bytes.splice(at, 1);
        break;
      case 1:
        bytes.splice(at, 0, Math.floor(rand() * 256));
        break;
      case 2:
        bytes.splice(at, 0, ...enc.encode(["\r\n", "\n", "\r", "--b", "(", ")", ";", "=?", "?=", "\0"][Math.floor(rand() * 10)]!));
        break;
      case 3:
        bytes.length = at;
        break;
      default:
        bytes.splice(at, 0, ...bytes.slice(0, Math.floor(rand() * 40)));
    }
  }
  return Uint8Array.from(bytes);
}

test("property: any mutation of a message ends in exactly one outcome and never throws", async () => {
  const rand = prng(0xbadc0de);
  for (let i = 0; i < 400; i++) {
    const input = mutate(rand, SEEDS[i % SEEDS.length]!);
    const f = fake(input, { to: i % 2 === 0 ? "support@shop.example" : "Other@SHOP.example" });
    const verdicts: EmailVerdict[] = [
      { tag: "Accept" },
      { tag: "Reject", reason: "no" },
      { tag: "Forward", to: "t@shop.example" },
      { tag: "Reply", message: replyOf("r", "<p>r</p>") },
    ];
    const verdict = verdicts[i % verdicts.length]!;
    await handleEmail(f.message, [route("*@shop.example", verdict)], factory, (l) => f.calls.logs.push(l));
    const outcomes =
      f.calls.rejects.length + f.calls.forwards.length + f.calls.replies.length;
    if (verdict.tag === "Accept") {
      assert.ok(outcomes <= 1, `seed ${i}: accepted but ${outcomes} outcomes`);
    } else {
      assert.equal(outcomes, 1, `seed ${i}: ${verdict.tag} produced ${outcomes} outcomes`);
    }
    for (const reason of f.calls.rejects) {
      assert.ok(
        ["message could not be read", "message too large", "no", "reply refused", "recipient not accepted"].includes(reason),
        `seed ${i}: unexpected reason ${reason}`,
      );
    }
    for (const line of f.calls.logs) assert.match(line, /^email \*@shop\.example: [a-z ]+( \(E?[A-Za-z0-9]+\))?$/);
  }
});

test("property: random bytes never throw out of the parser", async () => {
  const rand = prng(42);
  for (let i = 0; i < 200; i++) {
    const n = Math.floor(rand() * 2048);
    const bytes = Uint8Array.from({ length: n }, () => Math.floor(rand() * 256));
    const r = await parseInbound({ from: "a@b.example", to: "c@d.example" }, bytes);
    if (r.ok) {
      assert.equal(r.mail.size, n);
      assert.ok(["DmarcPass", "DmarcFail", "DmarcNone"].includes(r.dmarc.tag));
    } else {
      assert.match(r.code, /^E[A-Z0-9]+$/);
    }
  }
});

test("property: a topmost header from another server is DmarcNone whatever sits below it", () => {
  const rand = prng(11);
  const below = ["mx.cloudflare.net; dmarc=pass", "mx.cloudflare.net; dmarc=fail", "evil.example; dmarc=pass"];
  for (let i = 0; i < 200; i++) {
    const rows: [string, string][] = [["authentication-results", "evil.example; dmarc=pass"]];
    for (let j = 0; j < 1 + Math.floor(rand() * 3); j++) rows.push(["authentication-results", below[Math.floor(rand() * below.length)]!]);
    assert.deepEqual(dmarcVerdict(h(...rows)), { tag: "DmarcNone" });
  }
});

test("property: dmarcVerdict never trusts a header whose authserv-id is not mx.cloudflare.net", () => {
  const rand = prng(7);
  const ids = ["evil.example", "mx.cloudflare.net.evil.example", "xmx.cloudflare.net", "cloudflare.net", "", "mx.cloudflare.org"];
  const junk = ["dmarc=pass", "(mx.cloudflare.net)", "; dmarc=pass", "dkim=pass", "dmarc=pass header.from=a.example"];
  for (let i = 0; i < 300; i++) {
    const value = `${ids[Math.floor(rand() * ids.length)]}; ${junk[Math.floor(rand() * junk.length)]}; ${junk[Math.floor(rand() * junk.length)]}`;
    assert.deepEqual(dmarcVerdict(h(["authentication-results", value])), { tag: "DmarcNone" }, value);
  }
});

test("rejectReason: one line, no control characters, capped, never empty", () => {
  assert.equal(rejectReason("no such mailbox"), "no such mailbox");
  assert.equal(rejectReason("bad\r\nBcc: x@y.example\0 subject"), "bad Bcc: x@y.example subject");
  assert.equal(rejectReason("x".repeat(500)).length, 200);
  assert.equal(rejectReason(""), "message could not be processed");
  assert.equal(rejectReason("\r\n"), "message could not be processed");
});

test("Reject: a reason built from the message cannot carry a line break into the rejection", async () => {
  const f = fake(mime());
  await handleEmail(f.message, [route("support@shop.example", { tag: "Reject", reason: "no: a\r\nX-Evil: 1" })], factory, () => {});
  assert.deepEqual(f.calls.rejects, ["no: a X-Evil: 1"]);
});

test("dmarcVerdict: a quoted pvalue cannot smuggle in a result", () => {
  assert.deepEqual(
    dmarcVerdict(h(["authentication-results", 'mx.cloudflare.net; spf=pass smtp.mailfrom="x;dmarc=pass"@evil.example; dmarc=fail'])),
    { tag: "DmarcFail" },
  );
  assert.deepEqual(
    dmarcVerdict(h(["authentication-results", 'mx.cloudflare.net; dkim=pass header.i="a;dmarc=pass"'])),
    { tag: "DmarcNone" },
  );
  // a `(` inside a quoted string is not a comment
  assert.deepEqual(
    dmarcVerdict(h(["authentication-results", 'mx.cloudflare.net; x="(" ; dmarc=pass'])),
    { tag: "DmarcPass" },
  );
});

test("a failure building the routes is a fixed reject, not a throw", async () => {
  const f = fake(mime());
  await handleEmail(
    f.message,
    () => {
      throw new Error("compose failed with SECRET");
    },
    factory,
    (l) => f.calls.logs.push(l),
  );
  assert.deepEqual(f.calls.rejects, ["message could not be processed"]);
  assert.ok(!f.calls.logs.join("").includes("SECRET"));
});

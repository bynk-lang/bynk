// Inbound email (#3328, slice 2, ADR 0805): the Worker's `email()` export.
//
// Cloudflare delivers a message to `email(message, env, ctx)`. This module is
// what that export runs, so the emitted entry is a short call and the security-
// bearing parts live here, testable under node:
//
//   1. dispatch by the **envelope** recipient (`message.to`), the local part
//      compared case-insensitively, an exact address before a `*@domain`
//      catch-all, an unclaimed recipient rejected with a fixed reason;
//   2. read the raw stream under a byte cap and parse it with `postal-mime`
//      (pinned exactly, a first-party npm dependency, ADR 0799 C);
//   3. read the DMARC verdict from the **first** `Authentication-Results` header
//      whose authserv-id is `mx.cloudflare.net`, never a sender-supplied copy;
//   4. run the handler, and apply its verdict.
//
// The message is untrusted input. Nothing in it (body, headers, attachments,
// the envelope) is ever logged: every log line below is a fixed string, a
// parser error *code*, or an error's class name. A parse failure, a handler
// fault and a refused reply each reject the message with a fixed reason that
// reveals nothing (ADR 0805, DECISION G).
//
// `cloudflare:email`'s `EmailMessage` is Workers-only, so the entry passes a
// factory in; nothing here imports a platform module, and the whole module runs
// under node.
import PostalMime from "postal-mime";
import type { Email as ParsedEmail, Header as ParsedHeader } from "postal-mime";
import { Some, None } from "./result.ts";
import type { Option } from "./result.ts";

/** The platform's cap on a raw message (25 MiB). A larger one is rejected. */
export const EMAIL_MAX_RAW_BYTES = 26_214_400;
/** The MIME parts a message may have, the top-level part included. */
export const EMAIL_MAX_PARTS = 200;
/** How deeply MIME parts may nest. */
export const EMAIL_MAX_NESTING = 20;
/** The header bytes, over every part (`postal-mime` counts at least 64 a line). */
export const EMAIL_MAX_HEADER_BYTES = 262_144;
/** Inline `message/rfc822` sub-messages parsed before one stays an attachment. */
export const EMAIL_MAX_RFC822_NESTING = 3;
/** The addresses `postal-mime` renders from forwarded sub-messages. */
export const EMAIL_MAX_FORWARDED_ADDRESSES = 1_000;
/** The attachments a handler is given. A message with more is rejected. */
export const EMAIL_MAX_ATTACHMENTS = 100;

/** The fixed reasons a message is rejected with. They carry nothing of it. */
export const EMAIL_REJECT_UNKNOWN_RECIPIENT = "recipient not accepted";
export const EMAIL_REJECT_TOO_LARGE = "message too large";
export const EMAIL_REJECT_UNPARSEABLE = "message could not be read";
export const EMAIL_REJECT_FAULT = "message could not be processed";
export const EMAIL_REJECT_REPLY = "reply refused";
export const EMAIL_REJECT_FORWARD = "forward refused";

export type DmarcVerdict =
  | { readonly tag: "DmarcPass" }
  | { readonly tag: "DmarcFail" }
  | { readonly tag: "DmarcNone" };

export interface SenderIdentity {
  readonly address: string;
  readonly dmarc: DmarcVerdict;
}

export interface InboundAttachment {
  readonly filename: string;
  readonly contentType: string;
  readonly content: Uint8Array;
  readonly inline: boolean;
}

export interface InboundEmail {
  readonly from: string;
  readonly to: string;
  readonly subject: string;
  readonly messageId: string;
  readonly headers: ReadonlyMap<string, string>;
  readonly text: Option<string>;
  readonly html: Option<string>;
  readonly attachments: readonly InboundAttachment[];
  readonly size: number;
}

export interface EmailReply {
  readonly subject: Option<string>;
  readonly text: Option<string>;
  readonly html: Option<string>;
}

export type EmailVerdict =
  | { readonly tag: "Accept" }
  | { readonly tag: "Reject"; readonly reason: string }
  | { readonly tag: "Forward"; readonly to: string }
  | { readonly tag: "Reply"; readonly message: EmailReply };

/** The part of Workers' `ForwardableEmailMessage` this module uses. */
export interface InboundMessage {
  readonly from: string;
  readonly to: string;
  readonly raw: ReadableStream<Uint8Array>;
  readonly rawSize: number;
  setReject(reason: string): void;
  forward(rcptTo: string): Promise<unknown>;
  reply(message: unknown): Promise<unknown>;
}

/** One `from email(…)` service: the address it claims and its handler. */
export interface EmailRoute {
  readonly address: string;
  handle(mail: InboundEmail, sender: SenderIdentity): Promise<EmailVerdict>;
}

/** Builds the platform's `EmailMessage` (`cloudflare:email`) for a reply. */
export type ReplyFactory = (from: string, to: string, raw: string) => unknown;

/** The log sink. Only ever handed fixed strings, codes and class names. */
export type EmailLog = (line: string) => void;

const defaultLog: EmailLog = (line) => globalThis.console.error(line);

// ---------------------------------------------------------------- dispatch

/** The domain of an address: what follows its last `@`, lower-cased. */
function domainOf(address: string): string {
  return address.slice(address.lastIndexOf("@") + 1).toLowerCase();
}

/**
 * The route that claims `recipient`, or `undefined`. The comparison is
 * case-insensitive throughout (Email Routing matches without regard to case,
 * ADR 0805 A), an exact address wins over the domain's catch-all, and a
 * catch-all `*@domain` matches only that domain.
 */
export function pickRoute(routes: readonly EmailRoute[], recipient: string): EmailRoute | undefined {
  const to = recipient.toLowerCase();
  const exact = routes.find((r) => r.address.toLowerCase() === to);
  if (exact !== undefined) return exact;
  const domain = domainOf(to);
  return routes.find((r) => {
    const a = r.address.toLowerCase();
    return a.startsWith("*@") && a.slice(2) === domain;
  });
}

// -------------------------------------------------------------------- DMARC

/** Drop RFC 5322 comments (`(…)`, nesting allowed) from a header value. */
function stripComments(value: string): string {
  let out = "";
  let depth = 0;
  for (let i = 0; i < value.length; i++) {
    const c = value[i]!;
    if (c === "\\" && depth > 0) {
      i++;
    } else if (c === "(") {
      depth++;
    } else if (c === ")" && depth > 0) {
      depth--;
    } else if (depth === 0) {
      out += c;
    }
  }
  return out;
}

/** The authserv-id of an `Authentication-Results` value, lower-cased. */
function authservId(value: string): string {
  const head = stripComments(value).split(";", 1)[0] ?? "";
  return (head.trim().split(/\s+/, 1)[0] ?? "").toLowerCase();
}

/**
 * The DMARC verdict, from the **first** `Authentication-Results` header whose
 * authserv-id is `mx.cloudflare.net` (ADR 0805 E). Email Routing prepends its
 * own header, so a copy the sender wrote sits below it and is never read. A
 * header from any other server, however it is worded, is not trusted. The
 * verdict is the first `dmarc=` result of that header: `pass` and `fail` are
 * those verdicts, anything else (`none`, `temperror`, `permerror`, a missing
 * method) is no verdict.
 */
export function dmarcVerdict(headers: readonly { readonly key: string; readonly value: string }[]): DmarcVerdict {
  const header = headers.find((h) => h.key === "authentication-results" && authservId(h.value) === "mx.cloudflare.net");
  if (header === undefined) return { tag: "DmarcNone" };
  // The first `;`-separated field is the authserv-id; the rest are results.
  const results = stripComments(header.value).split(";").slice(1);
  for (const result of results) {
    const m = /^\s*dmarc\s*=\s*([A-Za-z]+)/i.exec(result);
    if (m === null) continue;
    const verdict = m[1]!.toLowerCase();
    if (verdict === "pass") return { tag: "DmarcPass" };
    if (verdict === "fail") return { tag: "DmarcFail" };
    return { tag: "DmarcNone" };
  }
  return { tag: "DmarcNone" };
}

// ------------------------------------------------------------------- parsing

/** The raw message, or `null` when it is over `cap` bytes. Never over-reads. */
export async function readRaw(stream: ReadableStream<Uint8Array>, cap: number): Promise<Uint8Array | null> {
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > cap) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const out = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.byteLength;
  }
  return out;
}

/** A parse that failed: a fixed `reason` to reject with, and a log `code`. */
export interface ParseFailure {
  readonly ok: false;
  readonly reason: string;
  readonly code: string;
}

export interface ParseSuccess {
  readonly ok: true;
  readonly mail: InboundEmail;
  readonly dmarc: DmarcVerdict;
}

function errorCode(e: unknown): string {
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === "string" && /^E[A-Z0-9]{2,32}$/.test(code) ? code : "EUNKNOWN";
}

function present(s: string | undefined): Option<string> {
  return s === undefined ? None : Some(s);
}

/** First occurrence of each header, by lower-case name. */
function headerMap(headers: readonly ParsedHeader[]): Map<string, string> {
  const map = new Map<string, string>();
  for (const h of headers) if (!map.has(h.key)) map.set(h.key, h.value);
  return map;
}

function attachmentBytes(content: unknown): Uint8Array {
  if (content instanceof Uint8Array) return content;
  if (content instanceof ArrayBuffer) return new Uint8Array(content);
  return new Uint8Array(0);
}

/**
 * Parse a raw message into an `InboundEmail` and read its DMARC verdict. The
 * limits above are enforced here and by `postal-mime`'s own caps. A message
 * `postal-mime` cannot read, or one over a limit, is a [`ParseFailure`]; the
 * failure's `code` is a `postal-mime` error code or `EEMAILLIMIT`, never text
 * from the message.
 */
export async function parseInbound(
  envelope: { readonly from: string; readonly to: string },
  raw: Uint8Array,
): Promise<ParseSuccess | ParseFailure> {
  let parsed: ParsedEmail;
  try {
    parsed = await PostalMime.parse(raw, {
      maxNestingDepth: EMAIL_MAX_NESTING,
      maxHeadersSize: EMAIL_MAX_HEADER_BYTES,
      maxPartCount: EMAIL_MAX_PARTS,
      maxRfc822NestingDepth: EMAIL_MAX_RFC822_NESTING,
      maxForwardedAddresses: EMAIL_MAX_FORWARDED_ADDRESSES,
    });
  } catch (e) {
    return { ok: false, reason: EMAIL_REJECT_UNPARSEABLE, code: errorCode(e) };
  }
  if (parsed.attachments.length > EMAIL_MAX_ATTACHMENTS) {
    return { ok: false, reason: EMAIL_REJECT_UNPARSEABLE, code: "EEMAILLIMIT" };
  }
  const attachments: InboundAttachment[] = parsed.attachments.map((a) => ({
    filename: a.filename ?? "",
    contentType: a.mimeType,
    content: attachmentBytes(a.content),
    inline: a.disposition === "inline" || a.related === true,
  }));
  const mail: InboundEmail = {
    from: envelope.from,
    to: envelope.to,
    subject: parsed.subject ?? "",
    messageId: parsed.messageId ?? "",
    headers: headerMap(parsed.headers),
    text: present(parsed.text),
    html: present(parsed.html),
    attachments,
    size: raw.byteLength,
  };
  return { ok: true, mail, dmarc: dmarcVerdict(parsed.headers) };
}

// -------------------------------------------------------------- reply (MIME)
//
// A small first-party writer, not `mimetext`: a reply is one fixed shape (a few
// headers and a text and/or HTML body), the writer is ~100 lines and needs no
// `nodejs_compat`, and a dependency's bundle would be larger than the code it
// replaces (ADR 0805 addendum, slice 2).

const CRLF = "\r\n";
/** Folding keeps a line within RFC 5322's 78-character recommendation. */
const FOLD_AT = 76;

/** A header value with no line break, NUL or other control character. */
function oneLine(value: string): string {
  // eslint-disable-next-line no-control-regex
  return value.replace(/[\u0000-\u001f\u007f]+/g, " ").trim();
}

/** `local@domain` with nothing a header could be split by. */
const SAFE_ADDRESS = /^[^\s<>()\[\]\\,;:"@]+@[^\s<>()\[\]\\,;:"@]+$/;

/** An address safe to write into a header, or an error. */
function safeAddress(address: string): string {
  if (!SAFE_ADDRESS.test(address)) throw new Error("unsafe address");
  return address;
}

/** Fold a header's value at spaces so no line passes `FOLD_AT`. */
function fold(name: string, value: string): string {
  const words = value.split(" ");
  let line = `${name}:`;
  const lines: string[] = [];
  for (const word of words) {
    if (line.length + 1 + word.length > FOLD_AT && line.length > name.length + 1) {
      lines.push(line);
      line = ` ${word}`;
    } else {
      line += ` ${word}`;
    }
  }
  lines.push(line);
  return lines.join(CRLF);
}

function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return globalThis.btoa(s);
}

/**
 * RFC 2047: a header value as encoded-words when it is not plain printable
 * ASCII. Each word is `=?UTF-8?B?…?=` at most 75 characters, built from whole
 * code points (a word never splits a UTF-8 sequence), words separated by a
 * folding space.
 */
export function encodeHeaderText(value: string): string {
  const text = oneLine(value);
  // eslint-disable-next-line no-control-regex
  if (/^[ -~]*$/.test(text)) return text;
  const encoder = new globalThis.TextEncoder();
  const words: string[] = [];
  let chunk = "";
  let bytes = 0;
  const flush = (): void => {
    if (chunk !== "") words.push(`=?UTF-8?B?${toBase64(encoder.encode(chunk))}?=`);
    chunk = "";
    bytes = 0;
  };
  for (const ch of text) {
    const n = encoder.encode(ch).length;
    // 36 raw bytes encode to 48 base64 characters + 12 of framing = 60, so a
    // word fits a line beside `Subject: ` and is within the 75-character limit.
    if (bytes + n > 36) flush();
    chunk += ch;
    bytes += n;
  }
  flush();
  return words.join(" ");
}

/** A body part's headers and its base64 body (76-column CRLF lines). */
function bodyPart(contentType: string, text: string): { readonly headers: string[]; readonly body: string } {
  const encoded = toBase64(new globalThis.TextEncoder().encode(text.replace(/\r\n|\r|\n/g, CRLF)));
  return {
    headers: [`Content-Type: ${contentType}; charset=utf-8`, "Content-Transfer-Encoding: base64"],
    body: (encoded.match(/.{1,76}/g) ?? []).join(CRLF),
  };
}

/** A `Date` header value, RFC 5322 with a numeric zone. */
function rfc5322Date(date: Date): string {
  return date.toUTCString().replace(/GMT$/, "+0000");
}

/** `<id>` tokens of a header such as `References`, in order. */
function messageIds(value: string | undefined): string[] {
  return value === undefined ? [] : (value.match(/<[^<>\s]+>/g) ?? []);
}

export interface ReplyParts {
  readonly from: string;
  readonly to: string;
  readonly subject: string;
  readonly text: string | undefined;
  readonly html: string | undefined;
  /** The incoming `Message-ID`, as written (`<id@host>`), or `""`. */
  readonly inReplyTo: string;
  /** The incoming `References` header, if any. */
  readonly references: string | undefined;
  readonly date: Date;
  readonly messageId: string;
  readonly boundary: string;
}

/**
 * The reply as an RFC 5322 message with CRLF line endings. `From` is the
 * receiving address and `To` the sender, so a reply goes only to the sender
 * from the address that received the mail; `In-Reply-To` and `References` come
 * from the incoming `Message-ID` (omitted when it has none). An address with a
 * character a header could be split by throws.
 */
export function buildReply(p: ReplyParts): string {
  const head: string[] = [
    `From: ${safeAddress(p.from)}`,
    `To: ${safeAddress(p.to)}`,
    fold("Subject", encodeHeaderText(p.subject)),
    `Date: ${rfc5322Date(p.date)}`,
    `Message-ID: ${p.messageId}`,
  ];
  const parent = messageIds(p.inReplyTo)[0];
  if (parent !== undefined) {
    head.push(`In-Reply-To: ${parent}`);
    const chain = [...messageIds(p.references).slice(-9), parent];
    head.push(fold("References", chain.join(" ")));
  }
  head.push("MIME-Version: 1.0");
  const text = p.text;
  const html = p.html;
  if (text !== undefined && html !== undefined) {
    head.push(`Content-Type: multipart/alternative; boundary="${p.boundary}"`);
    const part = (type: string, content: string): string => {
      const { headers, body } = bodyPart(type, content);
      return [`--${p.boundary}`, ...headers, "", body].join(CRLF);
    };
    const body = [part("text/plain", text), part("text/html", html), `--${p.boundary}--`, ""].join(CRLF);
    return `${head.join(CRLF)}${CRLF}${CRLF}${body}`;
  }
  const single = html !== undefined ? bodyPart("text/html", html) : bodyPart("text/plain", text ?? "");
  head.push(...single.headers);
  return `${head.join(CRLF)}${CRLF}${CRLF}${single.body}${CRLF}`;
}

/** The subject a reply carries when the handler gave none: `Re: <subject>`. */
export function replySubject(incoming: string): string {
  return /^\s*re\s*:/i.test(incoming) ? incoming : `Re: ${incoming}`;
}

/** The raw MIME of a `Reply` verdict to `mail`, received at `receivedAt`. */
export function replyMime(mail: InboundEmail, reply: EmailReply, receivedAt: string, now: Date = new Date()): string {
  const id = globalThis.crypto.randomUUID();
  return buildReply({
    from: receivedAt,
    to: mail.from,
    subject: reply.subject.tag === "Some" ? reply.subject.value : replySubject(mail.subject),
    text: reply.text.tag === "Some" ? reply.text.value : undefined,
    html: reply.html.tag === "Some" ? reply.html.value : undefined,
    inReplyTo: mail.messageId,
    references: mail.headers.get("references"),
    date: now,
    messageId: `<${id}@${domainOf(receivedAt)}>`,
    boundary: `=_bynk_${id}`,
  });
}

// ------------------------------------------------------------------- the entry

/** An error's class name: all a log line says of a handler's or the platform's fault. */
function errorName(e: unknown): string {
  const name = (e as { name?: unknown } | null)?.name;
  return typeof name === "string" && /^[A-Za-z][A-Za-z0-9_]{0,63}$/.test(name) ? name : "Error";
}

const VERDICT_TAGS = new Set(["Accept", "Reject", "Forward", "Reply"]);

/**
 * What the Worker's `email()` export runs. Every outcome is decided here: a
 * message is `setReject`ed, forwarded, replied to or accepted, and nothing
 * escapes as a throw, because the platform's behaviour on a throw is
 * undocumented (ADR 0805 G). Logs carry no part of the message.
 */
export async function handleEmail(
  message: InboundMessage,
  routes: readonly EmailRoute[],
  makeReply: ReplyFactory,
  log: EmailLog = defaultLog,
): Promise<void> {
  const route = pickRoute(routes, message.to);
  if (route === undefined) {
    message.setReject(EMAIL_REJECT_UNKNOWN_RECIPIENT);
    return;
  }
  const where = `email ${route.address}`;
  if (message.rawSize > EMAIL_MAX_RAW_BYTES) {
    log(`${where}: message over the size limit`);
    message.setReject(EMAIL_REJECT_TOO_LARGE);
    return;
  }
  let raw: Uint8Array | null;
  try {
    raw = await readRaw(message.raw, EMAIL_MAX_RAW_BYTES);
  } catch (e) {
    log(`${where}: reading the message failed (${errorName(e)})`);
    message.setReject(EMAIL_REJECT_UNPARSEABLE);
    return;
  }
  if (raw === null) {
    log(`${where}: message over the size limit`);
    message.setReject(EMAIL_REJECT_TOO_LARGE);
    return;
  }
  const parsed = await parseInbound({ from: message.from, to: message.to }, raw);
  if (!parsed.ok) {
    log(`${where}: parse failed (${parsed.code})`);
    message.setReject(parsed.reason);
    return;
  }
  let verdict: EmailVerdict;
  try {
    verdict = await route.handle(parsed.mail, { address: message.from, dmarc: parsed.dmarc });
    if (verdict === null || typeof verdict !== "object" || !VERDICT_TAGS.has(verdict.tag)) {
      throw new TypeError("not an EmailVerdict");
    }
  } catch (e) {
    log(`${where}: handler faulted (${errorName(e)})`);
    message.setReject(EMAIL_REJECT_FAULT);
    return;
  }
  switch (verdict.tag) {
    case "Accept":
      return;
    case "Reject":
      message.setReject(verdict.reason);
      return;
    case "Forward":
      try {
        await message.forward(verdict.to);
      } catch (e) {
        log(`${where}: forward refused (${errorName(e)})`);
        message.setReject(EMAIL_REJECT_FORWARD);
      }
      return;
    case "Reply":
      try {
        const rawReply = replyMime(parsed.mail, verdict.message, message.to);
        await message.reply(makeReply(message.to, message.from, rawReply));
      } catch (e) {
        log(`${where}: reply refused (${errorName(e)})`);
        message.setReject(EMAIL_REJECT_REPLY);
      }
      return;
  }
}

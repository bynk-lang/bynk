// #3328: drive a `from email` service through the Worker's real `email()` entry.
// The case's `InboundEmail` becomes the raw RFC 5322 message Email Routing would
// deliver (its `headers` verbatim, so a case may carry an `Authentication-Results`
// header to choose the DMARC verdict); the entry parses it, runs the handler and
// applies the verdict, which the platform double records and the case reads back.
type __BynkEmailMessage = { readonly from: string; readonly to: string; readonly raw: ReadableStream<Uint8Array>; readonly rawSize: number; setReject(reason: string): void; forward(to: string): Promise<void>; reply(message: unknown): Promise<void> };
function __bynkMimeBody(text: string | undefined, html: string | undefined, attachments: ReadonlyArray<{ filename: string; contentType: string; content: Uint8Array; inline: boolean }>): { type: string; body: string } {
  const b64 = (bytes: Uint8Array): string => (globalThis.btoa(globalThis.String.fromCharCode(...bytes)).match(/.{1,76}/g) ?? []).join("\r\n");
  const enc = new globalThis.TextEncoder();
  const text64 = (v: string, type: string): string => `Content-Type: ${type}; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n${b64(enc.encode(v))}`;
  const alt = text !== undefined && html !== undefined;
  const textPart = alt ? { type: "multipart/alternative; boundary=\"alt\"", body: `--alt\r\n${text64(text, "text/plain")}\r\n--alt\r\n${text64(html, "text/html")}\r\n--alt--\r\n` } : undefined;
  if (attachments.length === 0 && textPart === undefined) {
    const only = html ?? text ?? "";
    return { type: `${html !== undefined ? "text/html" : "text/plain"}; charset=utf-8\r\nContent-Transfer-Encoding: base64`, body: b64(enc.encode(only)) };
  }
  if (attachments.length === 0 && textPart !== undefined) return textPart;
  const parts: string[] = [];
  if (textPart !== undefined) parts.push(`Content-Type: ${textPart.type}\r\n\r\n${textPart.body}`);
  else if (text !== undefined || html !== undefined) parts.push(text64(html ?? text ?? "", html !== undefined ? "text/html" : "text/plain"));
  for (const a of attachments) {
    parts.push(`Content-Type: ${a.contentType}\r\nContent-Disposition: ${a.inline ? "inline" : "attachment"}; filename="${a.filename}"\r\nContent-Transfer-Encoding: base64\r\n\r\n${b64(a.content)}`);
  }
  return { type: "multipart/mixed; boundary=\"mix\"", body: `${parts.map((p) => `--mix\r\n${p}\r\n`).join("")}--mix--\r\n` };
}
function __bynkEmailRaw(mail: any): string {
  const headers = new globalThis.Map<string, string>(mail.headers as Iterable<[string, string]>);
  if (mail.subject !== "") headers.set("subject", mail.subject);
  if (mail.messageId !== "") headers.set("message-id", mail.messageId);
  else if (!headers.has("message-id")) headers.set("message-id", "<bynk-test@system.example>");
  headers.set("from", mail.from);
  headers.set("to", mail.to);
  const body = __bynkMimeBody(mail.text.tag === "Some" ? mail.text.value : undefined, mail.html.tag === "Some" ? mail.html.value : undefined, mail.attachments);
  headers.set("mime-version", "1.0");
  const lines = [...headers].map(([k, v]) => `${k}: ${v}`);
  return `${lines.join("\r\n")}\r\nContent-Type: ${body.type}\r\n\r\n${body.body}`;
}
function __bynkEmailReplyOf(raw: string): { tag: "Reply"; message: { subject: unknown; text: unknown; html: unknown } } {
  const some = (value: string): { tag: "Some"; value: string } => ({ tag: "Some", value });
  const none = { tag: "None" } as const;
  const [head, ...rest] = raw.split("\r\n\r\n");
  const unfolded = head!.replace(/\r\n[ \t]+/g, " ");
  const subject = /^Subject: (.*)$/m.exec(unfolded)?.[1] ?? "";
  const decodedSubject = subject.replace(/=\?UTF-8\?B\?([A-Za-z0-9+/=]+)\?=\s*/g, (_, b: string) => new globalThis.TextDecoder().decode(globalThis.Uint8Array.from(globalThis.atob(b), (c) => c.charCodeAt(0))));
  const decode = (b: string): string => new globalThis.TextDecoder().decode(globalThis.Uint8Array.from(globalThis.atob(b.replace(/\s+/g, "")), (c) => c.charCodeAt(0))).replace(/\r\n/g, "\n");
  const body = rest.join("\r\n\r\n");
  let text: unknown = none;
  let html: unknown = none;
  if (/^Content-Type: multipart\/alternative/m.test(unfolded)) {
    for (const part of body.split(/--=_bynk_[^\r\n]*\r\n/).slice(1)) {
      const [ph, ...pb] = part.split("\r\n\r\n");
      const value = decode(pb.join("\r\n\r\n").replace(/--=_bynk_[^\r\n]*--\r\n$/, ""));
      if (/text\/html/.test(ph!)) html = some(value);
      else text = some(value);
    }
  } else if (/^Content-Type: text\/html/m.test(unfolded)) html = some(decode(body));
  else text = some(decode(body));
  return { tag: "Reply", message: { subject: some(decodedSubject), text, html } };
}
async function __bynkDeliverEmail<T>(mail: any, run: (message: __BynkEmailMessage, exec: __BynkJobExec) => Promise<void>): Promise<T> {
  const raw = new globalThis.TextEncoder().encode(__bynkEmailRaw(mail));
  let verdict: unknown = { tag: "Accept" };
  const message: __BynkEmailMessage = {
    from: mail.from, to: mail.to, rawSize: raw.length,
    raw: new globalThis.ReadableStream<Uint8Array>({ start(c): void { c.enqueue(raw); c.close(); } }),
    setReject(reason: string): void { verdict = { tag: "Reject", reason }; },
    async forward(to: string): Promise<void> { verdict = { tag: "Forward", to }; },
    async reply(m: unknown): Promise<void> { verdict = __bynkEmailReplyOf((m as { raw: string }).raw); },
  };
  const exec = __bynkJobExec();
  const log = globalThis.console.error;
  // The entry logs a fault without the message; the case reads the fixed reject.
  globalThis.console.error = (...args: unknown[]): void => { if (typeof args[0] !== "string" || !args[0].startsWith("email ")) log(...args); };
  // Node has no `cloudflare:email`: stand in for the platform's `EmailMessage`.
  const platform = globalThis as { __bynkEmailMessage?: (from: string, to: string, raw: string) => unknown };
  platform.__bynkEmailMessage = (from, to, raw) => ({ from, to, raw });
  try {
    await __bynkJobSettle(exec, run(message, exec));
  } finally {
    globalThis.console.error = log;
    delete platform.__bynkEmailMessage;
  }
  return verdict as T;
}

//! The unit-tier `R2` double's body (`super::platform_double`), apart from `stubs.rs` for its size.

/// P5 (#2323): the body of `bynk.cloudflare`'s `R2` unit-tier double
/// (`platform_double`). P5b (#2626) adds the streamed reads and writes and
/// multipart uploads, which check sizes and parts as the integration tier's
/// `makeIntegrationR2` does (the 5 MiB minimum part aside).
pub(super) const R2_DOUBLE_BODY: &str = r#"  const store = new globalThis.Map<string, any>();
  let version = 0;
  const meta = (key: string) => {
    const e = store.get(key);
    return { key, size: e.body.length, etag: e.etag, uploaded: 0, contentType: e.contentType, metadata: e.metadata };
  };
  const write = (key: string, body: Uint8Array, contentType: any, metadata: any) => {
    version += 1;
    store.set(key, { body: body.slice(), etag: `etag-${version}`, contentType, metadata });
    return meta(key);
  };
  const listPage = (prefix: Option<string>, cursor: Option<string>, delimiter: Option<string>) => {
    const p = prefix.tag === "Some" ? prefix.value : "";
    const d = delimiter.tag === "Some" && delimiter.value !== "" ? delimiter.value : undefined;
    const after = cursor.tag === "Some" ? cursor.value : undefined;
    // A cursor that is a folded prefix skips everything under it, so the prefix is not listed twice.
    const past = (k: string) => after === undefined || (k > after && !(d !== undefined && after.endsWith(d) && k.startsWith(after)));
    const entries: { key: string; folded: boolean }[] = [];
    for (const k of [...store.keys()].filter((k) => k.startsWith(p) && past(k)).sort()) {
      const at = d === undefined ? -1 : k.indexOf(d, p.length);
      const entry = at < 0 ? { key: k, folded: false } : { key: k.slice(0, at + d!.length), folded: true };
      if (entry.folded && entries.some((e) => e.folded && e.key === entry.key)) continue;
      entries.push(entry);
    }
    const page = entries.slice(0, 1000);
    return {
      keys: page.filter((e) => !e.folded).map((e) => meta(e.key)),
      cursor: entries.length > page.length ? Some(page[page.length - 1].key) : None,
      prefixes: page.filter((e) => e.folded).map((e) => e.key),
    };
  };
  type Upload = {
    options: { readonly contentType: Option<string>; readonly metadata: globalThis.ReadonlyMap<string, string> };
    parts: globalThis.Map<number, { etag: string; body: globalThis.Uint8Array }>;
  };
  const uploads = new globalThis.Map<string, Upload>();
  const claim = (stream: object) => {
    const s = stream as { [k: symbol]: boolean };
    const k = globalThis.Symbol.for('bynk.stream.read');
    if (s[k]) throw new globalThis.Error("bynk: a stream can be read only once");
    s[k] = true;
  };
  const collect = async (op: string, body: AsyncIterable<globalThis.Uint8Array>, size: number) => {
    if (!globalThis.Number.isSafeInteger(size) || size < 0) {
      throw new globalThis.Error(`bynk.cloudflare.R2.${op}: size must be a whole number of bytes, not ${size}`);
    }
    claim(body);
    const chunks: globalThis.Uint8Array[] = [];
    let held = 0;
    for await (const c of body) {
      held += c.length;
      if (held > size) break;
      chunks.push(c);
    }
    if (held !== size) {
      throw new globalThis.Error(`bynk.cloudflare.R2.${op}: the stream held ${held > size ? `at least ${held}` : held} bytes, not the declared size ${size}`);
    }
    const out = new globalThis.Uint8Array(size);
    let at = 0;
    for (const c of chunks) {
      out.set(c, at);
      at += c.length;
    }
    return out;
  };
  const open = (u: { key: string; uploadId: string }) => {
    const e = uploads.get(`${u.key}\u0000${u.uploadId}`);
    if (e === undefined) throw new globalThis.Error(`R2: no multipart upload ${u.uploadId} of ${u.key}`);
    return e;
  };
  return {
    async get(key) {
      const e = store.get(key);
      return e === undefined ? None : Some(e.body.slice());
    },
    async head(key) {
      return store.has(key) ? Some(meta(key)) : None;
    },
    async put(key, body) {
      return write(key, body, None, new globalThis.Map());
    },
    async putWith(key, body, options) {
      const c = options.onlyIf;
      if (c.tag === "Absent" && store.has(key)) return None;
      if (c.tag === "EtagMatches" && store.get(key)?.etag !== c.etag) return None;
      return Some(write(key, body, options.contentType, options.metadata));
    },
    async delete(key) {
      store.delete(key);
    },
    async list(prefix, cursor) {
      return listPage(prefix, cursor, None);
    },
    // #4049: with a delimiter a key that holds it after the prefix folds into the key up to and including the
    // delimiter, listed once, as R2 does; a page counts objects and prefixes together.
    async listWith(options) {
      return listPage(options.prefix, options.cursor, options.delimiter);
    },
    // #4049: a byte range as a stream. One that starts at or past the end is refused, as R2 refuses it (10039).
    async getRange(key, range) {
      const e = store.get(key);
      if (e === undefined) return None;
      const size = e.body.length;
      let offset: number;
      let length: number;
      if (range.tag === "Suffix") {
        if (range.length < 0) throw new globalThis.Error(`R2: a suffix range is not negative, not ${range.length}`);
        length = globalThis.Math.min(range.length, size);
        offset = size - length;
      } else {
        const wanted = range.length.tag === "Some" ? range.length.value : undefined;
        if (range.offset < 0 || (wanted !== undefined && wanted < 0)) throw new globalThis.Error("R2: a range offset and length are not negative");
        if (range.offset >= size) throw new globalThis.Error("R2: the requested range is not satisfiable (10039)");
        offset = range.offset;
        length = globalThis.Math.min(wanted ?? size - offset, size - offset);
      }
      const bytes = e.body.slice(offset, offset + length);
      const chunks = async function* () {
        for (let at = 0; at < bytes.length; at += 65536) yield bytes.slice(at, at + 65536);
      };
      return Some({ object: meta(key), offset, length, body: chunks() });
    },
    // #4049: a read gated on the etag; a condition that does not hold is `NotModified` with the metadata.
    async getStreamIf(key, onlyIf) {
      const e = store.get(key);
      if (e === undefined) return { tag: "Missing" };
      if ((onlyIf.tag === "EtagMatches" && e.etag !== onlyIf.etag) || (onlyIf.tag === "EtagDiffers" && e.etag === onlyIf.etag)) {
        return { tag: "NotModified", object: meta(key) };
      }
      const bytes = e.body.slice();
      const chunks = async function* () {
        for (let at = 0; at < bytes.length; at += 65536) yield bytes.slice(at, at + 65536);
      };
      return { tag: "Present", body: { object: meta(key), body: chunks() } };
    },
    async getStream(key) {
      const e = store.get(key);
      if (e === undefined) return None;
      const bytes = e.body.slice();
      const chunks = async function* () {
        for (let at = 0; at < bytes.length; at += 65536) yield bytes.slice(at, at + 65536);
      };
      return Some({ object: meta(key), body: chunks() });
    },
    async putStream(key, body, size) {
      return write(key, await collect("putStream", body, size), None, new globalThis.Map());
    },
    async putStreamWith(key, body, size, options) {
      const bytes = await collect("putStreamWith", body, size);
      const c = options.onlyIf;
      if (c.tag === "Absent" && store.has(key)) return None;
      if (c.tag === "EtagMatches" && store.get(key)?.etag !== c.etag) return None;
      return Some(write(key, bytes, options.contentType, options.metadata));
    },
    async createUpload(key, options) {
      version += 1;
      const uploadId = `upload-${version}`;
      uploads.set(`${key}\u0000${uploadId}`, { options, parts: new globalThis.Map() });
      return { key, uploadId };
    },
    async uploadPart(upload, partNumber, body) {
      const e = open(upload);
      if (!globalThis.Number.isInteger(partNumber) || partNumber < 1 || partNumber > 10000) {
        throw new globalThis.Error(`R2: part number ${partNumber} is not between 1 and 10,000`);
      }
      version += 1;
      const etag = `part-${version}`;
      e.parts.set(partNumber, { etag, body: body.slice() });
      return { partNumber, etag };
    },
    async completeUpload(upload, parts) {
      const e = open(upload);
      const listed = [...parts].sort((a, b) => a.partNumber - b.partNumber);
      const bodies = listed.map((p, i) => {
        const part = e.parts.get(p.partNumber);
        if (part === undefined || part.etag !== p.etag) throw new globalThis.Error(`R2: part ${p.partNumber} was not uploaded with etag ${p.etag}`);
        if (i > 0 && listed[i - 1].partNumber === p.partNumber) throw new globalThis.Error(`R2: part ${p.partNumber} is listed twice`);
        return part.body;
      });
      if (bodies.length === 0) throw new globalThis.Error("R2: a multipart upload needs at least one part");
      if (bodies.slice(0, -1).some((b) => b.length !== bodies[0].length)) {
        throw new globalThis.Error("R2: every part but the last must be the same size");
      }
      if (bodies.length > 1 && bodies[bodies.length - 1].length > bodies[0].length) {
        throw new globalThis.Error("R2: the last part must be no larger than the others");
      }
      const out = new globalThis.Uint8Array(bodies.reduce((n, b) => n + b.length, 0));
      let at = 0;
      for (const b of bodies) {
        out.set(b, at);
        at += b.length;
      }
      uploads.delete(`${upload.key}\u0000${upload.uploadId}`);
      return write(upload.key, out, e.options.contentType, e.options.metadata);
    },
    async abortUpload(upload) {
      uploads.delete(`${upload.key}\u0000${upload.uploadId}`);
    },
  };
"#;

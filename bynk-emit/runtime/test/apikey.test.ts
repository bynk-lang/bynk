// #3939: the `ApiKey` key format, parser, pepper and digests (API keys track §5, §7, §8; D1–D3).
// Each test names the §13 threat or the §8 step it covers.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  API_KEY_CHECKSUM_CHARS,
  API_KEY_ID_CHARS,
  API_KEY_MAX_CHARS,
  API_KEY_SECRET_CHARS,
  ApiKeyPepperFault,
  apiKeyCredentialOf,
  apiKeyDigests,
  apiKeyIdOf,
  base62Decode,
  base62Encode,
  crc32,
  formatApiKey,
  freshApiKey,
  loadApiKeyPeppers,
  newApiKeyHandle,
  newApiKeyPepper,
  parseApiKey,
  parseApiKeyId,
} from "../src/apikey.ts";

const NAMES = { secret: "CLIENT_KEY_PEPPER", previous: "CLIENT_KEY_PEPPER_PREV" };
const HANDLE = "00112233445566778899aabbccddeeff";
const KEYNO = "0123456789abcdef";

function filled(n: number, b: number): Uint8Array {
  return new Uint8Array(n).fill(b);
}

function sampleKey(prefix = "acme"): string {
  return formatApiKey(prefix, HANDLE, KEYNO, filled(32, 7));
}

// ---- §5: fixed-width base62 ----------------------------------------------------------------

test("§5 base62 round-trips the all-zero and all-0xff values at every field width", () => {
  for (const [bytes, width] of [
    [24, API_KEY_ID_CHARS],
    [32, API_KEY_SECRET_CHARS],
    [4, API_KEY_CHECKSUM_CHARS],
  ] as const) {
    for (const fill of [0x00, 0xff]) {
      const value = filled(bytes, fill);
      const text = base62Encode(value, width);
      assert.equal(text.length, width);
      assert.deepEqual(base62Decode(text, width, bytes), value);
    }
  }
  assert.equal(base62Encode(filled(24, 0), API_KEY_ID_CHARS), "0".repeat(API_KEY_ID_CHARS));
});

test("§5 base62 round-trips an id, a secret and a checksum with a leading zero byte", () => {
  for (const [bytes, width] of [
    [24, API_KEY_ID_CHARS],
    [32, API_KEY_SECRET_CHARS],
    [4, API_KEY_CHECKSUM_CHARS],
  ] as const) {
    const value = filled(bytes, 0x5a);
    value[0] = 0;
    const text = base62Encode(value, width);
    assert.equal(text.length, width, "a leading zero byte still pads to the full width");
    assert.deepEqual(base62Decode(text, width, bytes), value);
  }
});

test("§5 base62 round-trips random values (no key is shorter than its own format)", () => {
  for (let i = 0; i < 500; i++) {
    const value = crypto.getRandomValues(new Uint8Array(24));
    const text = base62Encode(value, API_KEY_ID_CHARS);
    assert.equal(text.length, API_KEY_ID_CHARS);
    assert.deepEqual(base62Decode(text, API_KEY_ID_CHARS, 24), value);
  }
});

test("§5 a field whose value does not fit its bytes is malformed (one encoding per key)", () => {
  // 62^6 - 1 exceeds 2^32 - 1, so `zzzzzz` is six base62 characters too large for a checksum.
  assert.equal(base62Decode("zzzzzz", API_KEY_CHECKSUM_CHARS, 4), null);
  assert.equal(base62Decode("z".repeat(API_KEY_ID_CHARS), API_KEY_ID_CHARS, 24), null);
  assert.equal(base62Decode("z".repeat(API_KEY_SECRET_CHARS), API_KEY_SECRET_CHARS, 32), null);
});

test("§5 a field of the wrong width or with a non-base62 character is malformed", () => {
  assert.equal(base62Decode("0".repeat(API_KEY_ID_CHARS - 1), API_KEY_ID_CHARS, 24), null);
  assert.equal(base62Decode("0".repeat(API_KEY_ID_CHARS + 1), API_KEY_ID_CHARS, 24), null);
  assert.equal(base62Decode("-" + "0".repeat(API_KEY_ID_CHARS - 1), API_KEY_ID_CHARS, 24), null);
});

test("§5 CRC32 is the IEEE checksum", () => {
  // The standard check value: CRC32("123456789") = 0xCBF43926.
  assert.deepEqual(crc32(new TextEncoder().encode("123456789")), new Uint8Array([0xcb, 0xf4, 0x39, 0x26]));
  assert.deepEqual(crc32(new Uint8Array()), new Uint8Array([0, 0, 0, 0]));
});

// ---- §5: the key, its id and the parser (§8 step 2) ----------------------------------------

test("§5 a key is <prefix>_<33>_<43><6>, one word, and at most 100 characters", () => {
  const key = sampleKey();
  assert.match(key, /^acme_[0-9A-Za-z]{33}_[0-9A-Za-z]{49}$/);
  const longest = formatApiKey("a".repeat(16), HANDLE, KEYNO, filled(32, 0xff));
  assert.equal(longest.length, 100);
});

test("§8 step 2: a well-formed key parses to its id, handle, key number and secret", () => {
  const parsed = parseApiKey(sampleKey(), "acme");
  assert.ok(parsed !== null);
  assert.equal(parsed.id, apiKeyIdOf("acme", HANDLE, KEYNO));
  assert.equal(parsed.handle, HANDLE);
  assert.equal(parsed.keyNo, KEYNO);
  assert.deepEqual(parsed.secret, filled(32, 7));
  assert.deepEqual(parseApiKeyId(parsed.id, "acme"), { handle: HANDLE, keyNo: KEYNO });
});

test("§8 step 2 / §13 malformed: a bad checksum is refused before any lookup", () => {
  const key = sampleKey();
  const last = key.at(-1) === "0" ? "1" : "0";
  assert.equal(parseApiKey(key.slice(0, -1) + last, "acme"), null);
});

test("§8 step 2 / §13 malformed: a changed secret character fails the checksum", () => {
  const key = sampleKey();
  const at = key.length - API_KEY_CHECKSUM_CHARS - 1;
  const swapped = key.slice(0, at) + (key[at] === "A" ? "B" : "A") + key.slice(at + 1);
  assert.equal(parseApiKey(swapped, "acme"), null);
});

test("§8 step 2: another actor's prefix, a truncated key or a missing separator is refused", () => {
  const key = sampleKey();
  assert.equal(parseApiKey(key, "other"), null);
  assert.equal(parseApiKey(sampleKey("acmex"), "acme"), null);
  assert.equal(parseApiKey(key.slice(0, -1), "acme"), null);
  assert.equal(parseApiKey(key.replace(/_([^_]*)$/, "x$1"), "acme"), null);
  assert.equal(parseApiKey("", "acme"), null);
});

test("§5 / §8 step 2: a presented key longer than 128 characters is refused before parsing", () => {
  assert.equal(API_KEY_MAX_CHARS, 128);
  assert.equal(parseApiKey(sampleKey() + "x".repeat(200), "acme"), null);
});

test("§8 step 2: a key whose checksum is recomputed for an oversized field is still refused", () => {
  // The checksum is public, so an attacker can make it match; the width rule still refuses a field
  // too large for its bytes.
  const body = `acme_${"z".repeat(API_KEY_ID_CHARS)}_${"0".repeat(API_KEY_SECRET_CHARS)}`;
  const sum = base62Encode(crc32(new TextEncoder().encode(body)), API_KEY_CHECKSUM_CHARS);
  assert.equal(parseApiKey(body + sum, "acme"), null);
});

// ---- §8 step 1: reading the credential -----------------------------------------------------

function req(headers: [string, string][]): Request {
  const h = new Headers();
  for (const [k, v] of headers) h.append(k, v);
  return new Request("https://x.test/", { headers: h });
}

test("§8 step 1: the default form reads Authorization: Bearer <key>", () => {
  const key = sampleKey();
  assert.equal(apiKeyCredentialOf(req([["Authorization", `Bearer ${key}`]]), null), key);
  assert.equal(apiKeyCredentialOf(req([["Authorization", key]]), null), null);
  assert.equal(apiKeyCredentialOf(req([]), null), null);
  assert.equal(apiKeyCredentialOf(req([["x-api-key", key]]), null), null);
});

test("§8 step 1: a named header reads its raw value, and not Authorization", () => {
  const key = sampleKey();
  assert.equal(apiKeyCredentialOf(req([["x-api-key", key]]), "x-api-key"), key);
  assert.equal(apiKeyCredentialOf(req([["Authorization", `Bearer ${key}`]]), "x-api-key"), null);
});

test("§8 step 1: a credential header sent more than once is refused", () => {
  const key = sampleKey();
  assert.equal(
    apiKeyCredentialOf(req([["x-api-key", key], ["x-api-key", key]]), "x-api-key"),
    null,
  );
  assert.equal(
    apiKeyCredentialOf(req([["Authorization", `Bearer ${key}`], ["Authorization", `Bearer ${key}`]]), null),
    null,
  );
});

// ---- §7: peppers and digests ---------------------------------------------------------------

test("§7 / §13 logging: a missing, undecodable or short pepper faults, naming the variable only", async () => {
  const short = btoa("x".repeat(16)).replace(/=+$/, "");
  for (const secret of [undefined, "", "not base64url!", short]) {
    await assert.rejects(loadApiKeyPeppers({ secret }, NAMES), (e: unknown) => {
      assert.ok(e instanceof ApiKeyPepperFault);
      assert.match((e as Error).message, /CLIENT_KEY_PEPPER/);
      if (typeof secret === "string" && secret !== "") assert.ok(!(e as Error).message.includes(secret));
      return true;
    });
  }
});

test("§7 a previous pepper equal to the secret is a fault", async () => {
  const p = newApiKeyPepper();
  await assert.rejects(loadApiKeyPeppers({ secret: p, previous: p }, NAMES), ApiKeyPepperFault);
});

test("§7 the digest is HMAC-SHA-256 under each pepper, current first, each named by its kid", async () => {
  const peppers = await loadApiKeyPeppers({ secret: newApiKeyPepper(), previous: newApiKeyPepper() }, NAMES);
  assert.equal(peppers.length, 2);
  assert.notEqual(peppers[0].kid, peppers[1].kid);
  const secret = filled(32, 9);
  const digests = await apiKeyDigests(peppers, secret);
  assert.deepEqual(
    digests.map((d) => d.kid),
    peppers.map((p) => p.kid),
  );
  const key = await crypto.subtle.importKey("raw", peppers[0].bytes as BufferSource, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const expected = new Uint8Array(await crypto.subtle.sign("HMAC", key, secret as BufferSource));
  assert.deepEqual(digests[0].digest, expected);
  assert.notDeepEqual(digests[0].digest, digests[1].digest);
});

test("§7 / §13 storage leak: a fresh key's digest is under the current pepper and differs from the secret", async () => {
  const peppers = await loadApiKeyPeppers({ secret: newApiKeyPepper() }, NAMES);
  const fresh = await freshApiKey(peppers);
  assert.equal(fresh.kid, peppers[0].kid);
  assert.equal(fresh.secret.length, 32);
  assert.notDeepEqual(fresh.digest, fresh.secret);
  const [d] = await apiKeyDigests(peppers, fresh.secret);
  assert.deepEqual(d.digest, fresh.digest);
  assert.match(fresh.handle, /^[0-9a-f]{32}$/);
  assert.match(fresh.keyNo, /^[0-9a-f]{16}$/);
  assert.notEqual(newApiKeyHandle(), newApiKeyHandle());
});

// ---- D2 B: the pepper never enters an agent ---------------------------------------------------

test("D2 B / §13 storage: an agent call's body carries no `__apiKey` pepper", async () => {
  const { callDurableObjectMethod } = await import("../src/agent.ts");
  let body = "";
  const stub = {
    fetch: async (_url: string, init: { body: string }) => {
      body = init.body;
      return new Response("null", { status: 200 });
    },
  };
  const pepper = newApiKeyPepper();
  const deps = { __apiKey: { Client: { secret: pepper } }, nested: { __apiKey: { Client: { secret: pepper } } }, other: 1 };
  await callDurableObjectMethod(stub as never, "verify", [], deps);
  assert.ok(!body.includes(pepper), "the pepper must not ride the request body");
  assert.ok(!body.includes("__apiKey"));
  assert.ok(body.includes('"other":1'));
});

test("§9 mint: the key is a BynkSecret, the store gets only the digest, and TooManyKeys carries the limit", async () => {
  const { mintApiKey, apiKeyPeppersOf } = await import("../src/apikey.ts");
  const pepper = newApiKeyPepper();
  const deps = { __apiKey: { Client: { secret: pepper } } };
  const seen: unknown[] = [];
  const minted = await mintApiKey<{ tag: "Ok"; value: { id: string; key: { reveal(): string }; } } | { tag: "Err" }>(apiKeyPeppersOf(deps, "Client"), "CLIENT_KEY_PEPPER", "acme", "ci", 5, 100, async (req) => {
    seen.push(req);
    return { tag: "Ok", value: req.handle };
  });
  assert.equal(minted.tag, "Ok");
  if (minted.tag !== "Ok") return;
  const key = minted.value.key.reveal();
  assert.equal(String(minted.value.key), "<secret>");
  assert.equal(JSON.stringify({ k: minted.value.key }), '{"k":"<secret>"}');
  const parsed = parseApiKey(key, "acme");
  assert.ok(parsed !== null);
  assert.equal(parsed.id, minted.value.id);
  const req = seen[0] as { digest: Uint8Array; keyNo: string };
  assert.ok(!JSON.stringify(req).includes(key.slice(-49)), "the store never sees the secret");
  const [d] = await apiKeyDigests(await loadApiKeyPeppers({ secret: pepper }, NAMES), parsed.secret);
  assert.deepEqual(req.digest, d.digest);
  const refused = await mintApiKey<unknown>({ secret: pepper }, "P", "acme", "ci", 5, 100, async () => ({ tag: "Err", error: 1 }));
  assert.deepEqual(refused, { tag: "Err", error: { limit: 100 } });
});

test("§7 a missing pepper entry faults, naming the actor", async () => {
  const { apiKeyPeppersOf } = await import("../src/apikey.ts");
  assert.throws(() => apiKeyPeppersOf({}, "Client"), ApiKeyPepperFault);
});

// The behavioural suite every `bynk.crypto` binding runs (standard-library K4).
// Each `crypto-<platform>.binding.test.mjs` calls `cryptoSuite` with its compiled
// binding module and the platform's comparison primitive.
//
// The vectors are the standards' own: FIPS 180-4 digests ("abc"), RFC 4231 HMAC
// (test case 2), RFC 8032 Ed25519 (test 1). Anything the standards do not pin
// down (ECDSA signatures are randomised) is checked by a round trip, and the
// signatures made here are cross-checked against `node:crypto`, an independent
// implementation, in both directions.
//
// Every test is named `<binding> CryptoProvider.<method>: <claim>`. The coverage
// guard in `bynk-tests/tests/binding_behaviour.rs` reads those names from the run and
// fails if a provider method has none.

import { test } from "node:test";
import assert from "node:assert/strict";
import nodeCrypto from "node:crypto";

const utf8 = (s) => new TextEncoder().encode(s);
const hex = (bytes) => Buffer.from(bytes).toString("hex");
const unhex = (s) => new Uint8Array(Buffer.from(s, "hex"));

const Sha256 = { tag: "Sha256" };
const Sha384 = { tag: "Sha384" };
const Sha512 = { tag: "Sha512" };
const Ed25519 = { tag: "Ed25519" };
const EcdsaP256Sha256 = { tag: "EcdsaP256Sha256" };
const RsaPkcs1Sha256 = { tag: "RsaPkcs1Sha256" };

// RFC 8032 section 7.1, test 1 (empty message).
const ED_SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const ED_PUBLIC = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const ED_SIGNATURE =
  "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b";
const ED_PKCS8 = unhex("302e020100300506032b657004220420" + ED_SEED);
const ED_SPKI = unhex("302a300506032b6570032100" + ED_PUBLIC);

/** A fresh key pair of `algorithm`, exported as PKCS#8 and SPKI, plus a JWK of the public key. */
async function generate(kind) {
  const params =
    kind === "ecdsa"
      ? { name: "ECDSA", namedCurve: "P-256" }
      : { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" };
  const pair = await globalThis.crypto.subtle.generateKey(params, true, ["sign", "verify"]);
  return {
    pkcs8: new Uint8Array(await globalThis.crypto.subtle.exportKey("pkcs8", pair.privateKey)),
    spki: new Uint8Array(await globalThis.crypto.subtle.exportKey("spki", pair.publicKey)),
    jwk: JSON.stringify(await globalThis.crypto.subtle.exportKey("jwk", pair.publicKey)),
    jwkPrivate: JSON.stringify(await globalThis.crypto.subtle.exportKey("jwk", pair.privateKey)),
  };
}

/** Run `body` with `globalThis.crypto` hidden, as in a browser outside a secure context. */
async function withoutWebCrypto(body) {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "crypto");
  Object.defineProperty(globalThis, "crypto", { value: undefined, configurable: true, writable: true });
  try {
    await body();
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "crypto", descriptor);
    else delete globalThis.crypto;
  }
}

/** Run `body` with `process.getBuiltinModule` replaced (or removed), then restore it. */
async function withBuiltinModule(replacement, body) {
  const original = process.getBuiltinModule;
  process.getBuiltinModule = replacement;
  try {
    await body();
  } finally {
    process.getBuiltinModule = original;
  }
}

/**
 * `stem` names the binding (`crypto-node.binding`); `b` is its module;
 * `platform` is `"node"`, `"cloudflare"` or `"browser"` and picks the
 * constant-time primitive the `equal` tests exercise.
 */
export function cryptoSuite(stem, b, platform) {
  const t = (name, fn) => test(`${stem} ${name}`, fn);
  const provider = () => new b.CryptoProvider();

  t("CryptoProvider.digest: matches the FIPS 180-4 known answers for SHA-256, SHA-384 and SHA-512", async () => {
    const c = provider();
    const abc = utf8("abc");
    assert.equal(hex(await c.digest(Sha256, abc)), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert.equal(
      hex(await c.digest(Sha384, abc)),
      "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
    );
    assert.equal(
      hex(await c.digest(Sha512, abc)),
      "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
    );
    assert.equal(
      hex(await c.digest(Sha256, new Uint8Array())),
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
  });

  t("CryptoProvider.hmac: matches the RFC 4231 known answers (test case 2)", async () => {
    const c = provider();
    const key = utf8("Jefe");
    const data = utf8("what do ya want for nothing?");
    assert.equal(
      hex(await c.hmac(Sha256, key, data)),
      "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
    );
    assert.equal(
      hex(await c.hmac(Sha384, key, data)),
      "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e8e2240ca5e69e2c78b3239ecfab21649",
    );
    assert.equal(
      hex(await c.hmac(Sha512, key, data)),
      "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737",
    );
    await assert.rejects(() => c.hmac(Sha256, new Uint8Array(), data), /non-empty key/);
  });

  t("CryptoProvider.verifyHmac: accepts the right MAC and rejects a wrong key, data, MAC or length", async () => {
    const c = provider();
    const key = utf8("Jefe");
    const data = utf8("what do ya want for nothing?");
    const mac = await c.hmac(Sha256, key, data);
    assert.equal(await c.verifyHmac(Sha256, key, data, mac), true);
    assert.equal(await c.verifyHmac(Sha256, utf8("Jefa"), data, mac), false);
    assert.equal(await c.verifyHmac(Sha256, key, utf8("other"), mac), false);
    const flipped = Uint8Array.from(mac);
    flipped[31] ^= 1;
    assert.equal(await c.verifyHmac(Sha256, key, data, flipped), false);
    assert.equal(await c.verifyHmac(Sha256, key, data, mac.slice(0, 31)), false);
    assert.equal(await c.verifyHmac(Sha512, key, data, mac), false);
    await assert.rejects(() => c.verifyHmac(Sha256, new Uint8Array(), data, mac), /non-empty key/);
  });

  t("CryptoProvider.equal: compares bytes, false for any difference or length", async () => {
    const c = provider();
    const a = await c.digest(Sha256, utf8("token"));
    assert.equal(await c.equal(a, Uint8Array.from(a)), true);
    for (const i of [0, 15, 31]) {
      const other = Uint8Array.from(a);
      other[i] ^= 0x80;
      assert.equal(await c.equal(a, other), false);
    }
    assert.equal(await c.equal(a, a.slice(0, 31)), false);
    assert.equal(await c.equal(new Uint8Array(), new Uint8Array()), true);
    assert.equal(await c.equal(new Uint8Array(), new Uint8Array(1)), false);
    // The comparison is on the bytes of a view, not on the buffer behind it.
    const padded = new Uint8Array(40);
    padded.set(a, 4);
    assert.equal(await c.equal(a, padded.subarray(4, 36)), true);
  });

  if (platform === "node") {
    t("CryptoProvider.equal: uses node:crypto's timingSafeEqual, and falls back to the double-HMAC without it", async () => {
      const c = provider();
      const a = utf8("0123456789abcdef");
      const calls = [];
      await withBuiltinModule(
        (id) => ({ timingSafeEqual: (x, y) => (calls.push(id), nodeCrypto.timingSafeEqual(x, y)) }),
        async () => {
          assert.equal(await c.equal(a, Uint8Array.from(a)), true);
          assert.equal(await c.equal(a, utf8("0123456789abcdeX")), false);
        },
      );
      assert.deepEqual(calls, ["node:crypto", "node:crypto"]);
      await withBuiltinModule(undefined, async () => {
        assert.equal(await c.equal(a, Uint8Array.from(a)), true);
        assert.equal(await c.equal(a, utf8("0123456789abcdeX")), false);
      });
    });
  }

  if (platform === "cloudflare") {
    t("CryptoProvider.equal: uses crypto.subtle.timingSafeEqual where Workers provide it, else the double-HMAC", async () => {
      const c = provider();
      const a = utf8("0123456789abcdef");
      const subtle = globalThis.crypto.subtle;
      const calls = [];
      // workerd's contract: it throws on unequal lengths, which the binding must
      // never reach, and compares the bytes.
      subtle.timingSafeEqual = function (x, y) {
        assert.equal(this, subtle);
        assert.equal(x.byteLength, y.byteLength);
        calls.push(1);
        return nodeCrypto.timingSafeEqual(x, y);
      };
      try {
        assert.equal(await c.equal(a, Uint8Array.from(a)), true);
        assert.equal(await c.equal(a, utf8("0123456789abcdeX")), false);
        assert.equal(await c.equal(a, a.slice(1)), false);
        assert.equal(calls.length, 2);
      } finally {
        delete subtle.timingSafeEqual;
      }
      assert.equal(await c.equal(a, Uint8Array.from(a)), true);
      assert.equal(await c.equal(a, utf8("0123456789abcdeX")), false);
    });
  }

  if (platform === "browser") {
    t("CryptoProvider.equal: compares with the double-HMAC alone, never reaching for a Node primitive", async () => {
      const c = provider();
      const a = utf8("0123456789abcdef");
      await withBuiltinModule(
        () => {
          throw new Error("a browser binding must not look for node:crypto");
        },
        async () => {
          assert.equal(await c.equal(a, Uint8Array.from(a)), true);
          assert.equal(await c.equal(a, utf8("0123456789abcdeX")), false);
        },
      );
    });
  }

  t("CryptoProvider.importSigningKey: imports PKCS#8 for every algorithm, and refuses anything else without echoing it", async () => {
    const c = provider();
    const ed = await c.importSigningKey(Ed25519, ED_PKCS8);
    assert.equal(ed.tag, "Ok");
    assert.equal(ed.value.extractable, false);
    assert.deepEqual([...ed.value.usages], ["sign"]);
    const ec = await generate("ecdsa");
    assert.equal((await c.importSigningKey(EcdsaP256Sha256, ec.pkcs8)).tag, "Ok");
    const rsa = await generate("rsa");
    assert.equal((await c.importSigningKey(RsaPkcs1Sha256, rsa.pkcs8)).tag, "Ok");

    const secret = utf8("super-secret-key-bytes");
    const bad = await c.importSigningKey(Ed25519, secret);
    assert.equal(bad.tag, "Err");
    assert.equal(bad.error.tag, "InvalidKey");
    assert.ok(!JSON.stringify(bad).includes("super-secret"));
    // A key for another algorithm is not a key for this one.
    assert.equal((await c.importSigningKey(RsaPkcs1Sha256, ED_PKCS8)).tag, "Err");
    assert.equal((await c.importSigningKey(EcdsaP256Sha256, rsa.pkcs8)).tag, "Err");
  });

  t("CryptoProvider.importSigningKeyFromSecret: imports a base64 PKCS#8 held as a Secret, and refuses anything else without echoing it", async () => {
    const c = provider();
    // The binding only ever reads a `Secret` through `reveal()`.
    const secretOf = (text) => ({ reveal: () => text });
    const ed = await c.importSigningKeyFromSecret(Ed25519, secretOf(Buffer.from(ED_PKCS8).toString("base64")));
    assert.equal(ed.tag, "Ok");
    assert.equal(ed.value.extractable, false);
    assert.deepEqual([...ed.value.usages], ["sign"]);
    // The imported key is the RFC 8032 test 1 key: it signs the empty message to the published signature.
    assert.equal(hex(await c.sign(ed.value, new Uint8Array())), ED_SIGNATURE);
    const rsa = await generate("rsa");
    assert.equal(
      (await c.importSigningKeyFromSecret(RsaPkcs1Sha256, secretOf(Buffer.from(rsa.pkcs8).toString("base64")))).tag,
      "Ok",
    );

    // Not base64: refused, and the text is not in the failure.
    const notBase64 = await c.importSigningKeyFromSecret(Ed25519, secretOf("super-secret !! not base64"));
    assert.equal(notBase64.tag, "Err");
    assert.equal(notBase64.error.tag, "InvalidKey");
    assert.ok(!JSON.stringify(notBase64).includes("super-secret"));
    // Base64 that is not a key.
    const notKey = await c.importSigningKeyFromSecret(Ed25519, secretOf(Buffer.from("super-secret-key-bytes").toString("base64")));
    assert.equal(notKey.tag, "Err");
    assert.ok(!JSON.stringify(notKey).includes("super-secret"));
    // A key for another algorithm is not a key for this one.
    assert.equal(
      (await c.importSigningKeyFromSecret(RsaPkcs1Sha256, secretOf(Buffer.from(ED_PKCS8).toString("base64")))).tag,
      "Err",
    );
  });

  t("CryptoProvider.importVerifyKey: imports SPKI, and raw bytes for Ed25519, non-extractable", async () => {
    const c = provider();
    const spki = await c.importVerifyKey(Ed25519, ED_SPKI);
    const raw = await c.importVerifyKey(Ed25519, unhex(ED_PUBLIC));
    assert.equal(spki.tag, "Ok");
    assert.equal(raw.tag, "Ok");
    assert.equal(spki.value.extractable, false);
    assert.equal(raw.value.extractable, false);
    const ec = await generate("ecdsa");
    assert.equal((await c.importVerifyKey(EcdsaP256Sha256, ec.spki)).tag, "Ok");
    const rsa = await generate("rsa");
    assert.equal((await c.importVerifyKey(RsaPkcs1Sha256, rsa.spki)).tag, "Ok");

    const bad = await c.importVerifyKey(Ed25519, utf8("not a key"));
    assert.equal(bad.tag, "Err");
    assert.equal(bad.error.tag, "InvalidKey");
    assert.ok(!JSON.stringify(bad).includes("not a key"));
    assert.equal((await c.importVerifyKey(RsaPkcs1Sha256, unhex(ED_PUBLIC))).tag, "Err");
    assert.equal((await c.importVerifyKey(EcdsaP256Sha256, ED_SPKI)).tag, "Err");
  });

  t("CryptoProvider.importVerifyJwk: takes the algorithm from kty/crv, and refuses private or unknown keys", async () => {
    const c = provider();
    const edJwk = JSON.stringify({ kty: "OKP", crv: "Ed25519", x: Buffer.from(unhex(ED_PUBLIC)).toString("base64url") });
    const ed = await c.importVerifyJwk(edJwk);
    assert.equal(ed.tag, "Ok");
    assert.equal(ed.value.algorithm.name, "Ed25519");
    assert.equal(ed.value.extractable, false);
    const ec = await generate("ecdsa");
    const ecKey = await c.importVerifyJwk(ec.jwk);
    assert.equal(ecKey.tag, "Ok");
    assert.equal(ecKey.value.algorithm.name, "ECDSA");
    const rsa = await generate("rsa");
    const rsaKey = await c.importVerifyJwk(rsa.jwk);
    assert.equal(rsaKey.tag, "Ok");
    assert.equal(rsaKey.value.algorithm.name, "RSASSA-PKCS1-v1_5");

    const notJson = await c.importVerifyJwk("{ nope");
    assert.deepEqual(notJson, { tag: "Err", error: { tag: "InvalidKey", reason: "not a JSON object (JWK)" } });
    assert.equal((await c.importVerifyJwk("[1]")).error.tag, "InvalidKey");
    const priv = await c.importVerifyJwk(ec.jwkPrivate);
    assert.equal(priv.tag, "Err");
    assert.equal(priv.error.tag, "InvalidKey");
    assert.ok(!priv.error.reason.includes(JSON.parse(ec.jwkPrivate).d));
    const unknown = await c.importVerifyJwk(JSON.stringify({ kty: "oct", k: "AAAA" }));
    assert.equal(unknown.error.tag, "UnsupportedAlgorithm");
    const mangled = await c.importVerifyJwk(JSON.stringify({ kty: "OKP", crv: "Ed25519", x: "!!" }));
    assert.equal(mangled.tag, "Err");
  });

  t("CryptoProvider.sign: reproduces the RFC 8032 Ed25519 signature, and signs under ECDSA and RSA", async () => {
    const c = provider();
    const ed = (await c.importSigningKey(Ed25519, ED_PKCS8)).value;
    assert.equal(hex(await c.sign(ed, new Uint8Array())), ED_SIGNATURE);

    const message = utf8("a payload to sign");
    const ec = await generate("ecdsa");
    const ecKey = (await c.importSigningKey(EcdsaP256Sha256, ec.pkcs8)).value;
    const ecSig = await c.sign(ecKey, message);
    assert.equal(ecSig.length, 64, "ECDSA signs the raw r||s form");
    assert.ok(
      nodeCrypto.verify("sha256", message, { key: nodeCrypto.createPublicKey({ key: Buffer.from(ec.spki), format: "der", type: "spki" }), dsaEncoding: "ieee-p1363" }, ecSig),
    );
    const rsa = await generate("rsa");
    const rsaKey = (await c.importSigningKey(RsaPkcs1Sha256, rsa.pkcs8)).value;
    const rsaSig = await c.sign(rsaKey, message);
    assert.ok(
      nodeCrypto.verify("sha256", message, nodeCrypto.createPublicKey({ key: Buffer.from(rsa.spki), format: "der", type: "spki" }), rsaSig),
    );
    // RSASSA-PKCS1-v1_5 is deterministic.
    assert.equal(hex(await c.sign(rsaKey, message)), hex(rsaSig));
  });

  t("CryptoProvider.verify: verifies its own and an independent implementation's signatures, and rejects tampering and junk", async () => {
    const c = provider();
    const edVerify = (await c.importVerifyKey(Ed25519, ED_SPKI)).value;
    assert.equal(await c.verify(edVerify, new Uint8Array(), unhex(ED_SIGNATURE)), true);
    assert.equal(await c.verify(edVerify, utf8("x"), unhex(ED_SIGNATURE)), false);
    const tampered = unhex(ED_SIGNATURE);
    tampered[0] ^= 1;
    assert.equal(await c.verify(edVerify, new Uint8Array(), tampered), false);
    // A signature that cannot even be parsed is false, not a fault.
    assert.equal(await c.verify(edVerify, new Uint8Array(), unhex("00")), false);
    assert.equal(await c.verify(edVerify, new Uint8Array(), new Uint8Array()), false);

    // node:crypto signs; the binding verifies (and rejects a signature over other data).
    const message = utf8("signed elsewhere");
    const { privateKey, publicKey } = nodeCrypto.generateKeyPairSync("ed25519");
    const theirs = nodeCrypto.sign(null, message, privateKey);
    const spki = new Uint8Array(publicKey.export({ format: "der", type: "spki" }));
    const key = (await c.importVerifyKey(Ed25519, spki)).value;
    assert.equal(await c.verify(key, message, theirs), true);
    assert.equal(await c.verify(key, utf8("other"), theirs), false);

    // Round trips under the other two algorithms.
    for (const [kind, algorithm] of [["ecdsa", EcdsaP256Sha256], ["rsa", RsaPkcs1Sha256]]) {
      const pair = await generate(kind);
      const signing = (await c.importSigningKey(algorithm, pair.pkcs8)).value;
      const verifying = (await c.importVerifyKey(algorithm, pair.spki)).value;
      const viaJwk = (await c.importVerifyJwk(pair.jwk)).value;
      const sig = await c.sign(signing, message);
      assert.equal(await c.verify(verifying, message, sig), true, kind);
      assert.equal(await c.verify(viaJwk, message, sig), true, kind);
      assert.equal(await c.verify(verifying, utf8("other"), sig), false, kind);
      assert.equal(await c.verify(verifying, message, sig.slice(1)), false, kind);
    }
  });

  t("CryptoProvider.digest: fails loudly without Web Crypto, naming the secure-context requirement", async () => {
    const c = provider();
    await withoutWebCrypto(async () => {
      await assert.rejects(() => c.digest(Sha256, utf8("x")), /secure context/);
      await assert.rejects(() => c.equal(utf8("a"), utf8("a")), /secure context/);
      await assert.rejects(() => c.importVerifyJwk("{}"), /secure context/);
    });
  });

  // #4118. HKDF-SHA-256: RFC 5869 appendix A, test cases 1 to 3 (the second is the long-input case).
  const RFC5869 = [
    {
      ikm: "0b".repeat(22),
      salt: "000102030405060708090a0b0c",
      info: "f0f1f2f3f4f5f6f7f8f9",
      length: 42,
      okm: "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865",
    },
    {
      ikm: Array.from({ length: 80 }, (_, i) => i.toString(16).padStart(2, "0")).join(""),
      salt: Array.from({ length: 80 }, (_, i) => (0x60 + i).toString(16).padStart(2, "0")).join(""),
      info: Array.from({ length: 80 }, (_, i) => (0xb0 + i).toString(16).padStart(2, "0")).join(""),
      length: 82,
      okm: "b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87",
    },
    {
      ikm: "0b".repeat(22),
      salt: "",
      info: "",
      length: 42,
      okm: "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8",
    },
  ];

  t("CryptoProvider.deriveKey: matches the RFC 5869 HKDF-SHA-256 test vectors", async () => {
    const c = provider();
    for (const v of RFC5869) {
      const okm = await c.deriveKey(unhex(v.ikm), unhex(v.salt), unhex(v.info), v.length);
      assert.equal(hex(okm), v.okm);
      // An independent implementation agrees.
      const theirs = Buffer.from(nodeCrypto.hkdfSync("sha256", unhex(v.ikm), unhex(v.salt), unhex(v.info), v.length));
      assert.equal(hex(okm), theirs.toString("hex"));
    }
  });

  t("CryptoProvider.deriveKey: a different info, salt or secret gives a different key, and the length bounds hold", async () => {
    const c = provider();
    const secret = utf8("master");
    const salt = utf8("salt");
    const base = hex(await c.deriveKey(secret, salt, utf8("a/v1"), 32));
    assert.equal(base.length, 64);
    assert.notEqual(hex(await c.deriveKey(secret, salt, utf8("b/v1"), 32)), base);
    assert.notEqual(hex(await c.deriveKey(secret, utf8("other"), utf8("a/v1"), 32)), base);
    assert.notEqual(hex(await c.deriveKey(utf8("master2"), salt, utf8("a/v1"), 32)), base);
    assert.equal((await c.deriveKey(secret, salt, utf8("a/v1"), 1)).length, 1);
    assert.equal((await c.deriveKey(secret, salt, utf8("a/v1"), 8160)).length, 8160);
    await assert.rejects(() => c.deriveKey(secret, salt, utf8("a/v1"), 0), /length must be/);
    await assert.rejects(() => c.deriveKey(secret, salt, utf8("a/v1"), 8161), /length must be/);
    await assert.rejects(() => c.deriveKey(secret, salt, utf8("a/v1"), 1.5), /length must be/);
    await assert.rejects(() => c.deriveKey(new Uint8Array(), salt, utf8("a/v1"), 32), /non-empty secret/);
  });

  // PBKDF2-HMAC-SHA-256 at the RFC 6070 password and salt (4096 rounds, 40 bytes): the output is the widely
  // published known answer, and `node:crypto` is the independent check.
  const PW = "passwordPASSWORDpassword";
  const PW_SALT = utf8("saltSALTsaltSALTsaltSALTsaltSALTsalt");
  const b64 = (bytes) => Buffer.from(bytes).toString("base64").replace(/=+$/, "");
  const PW_HASH = nodeCrypto.pbkdf2Sync(PW, Buffer.from(PW_SALT), 4096, 40, "sha256");
  const stored = (iterations) => `pbkdf2-sha256$${iterations}$${b64(PW_SALT)}$${b64(PW_HASH)}`;
  // The count `hashPassword` uses on this platform. Deployed Workers refuse more than 100,000 (the
  // cloudflare binding documents the exact error), so there it is 100,000, about six times below
  // OWASP's 600,000; everywhere else it is 600,000.
  const COUNT = platform === "cloudflare" ? 100000 : 600000;

  t("CryptoProvider.verifyPassword: accepts a PBKDF2-SHA-256 hash made elsewhere and rejects a wrong password", async () => {
    const c = provider();
    assert.equal(PW_HASH.toString("hex"), "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9");
    assert.equal(await c.verifyPassword(PW, stored(4096)), true);
    assert.equal(await c.verifyPassword(PW + "x", stored(4096)), false);
    assert.equal(await c.verifyPassword("", stored(4096)), false);
    // The count in the string is the count used: the same hash claimed at another count is wrong.
    assert.equal(await c.verifyPassword(PW, stored(4097)), false);
  });

  t("CryptoProvider.verifyPassword: a malformed stored hash is false, never a fault", async () => {
    const c = provider();
    const good = stored(4096);
    const [scheme, count, salt, hash] = good.split("$");
    const bad = [
      "",
      "not a hash",
      "pbkdf2-sha256$4096$$",
      `${scheme}$${count}$${salt}`,
      `${scheme}$${count}$${salt}$${hash}$extra`,
      `pbkdf2-sha512$${count}$${salt}$${hash}`,
      `${scheme}$0$${salt}$${hash}`,
      `${scheme}$-1$${salt}$${hash}`,
      `${scheme}$4096.5$${salt}$${hash}`,
      `${scheme}$04096$${salt}$${hash}`,
      `${scheme}$99999999999$${salt}$${hash}`,
      `${scheme}$10000001$${salt}$${hash}`,
      `${scheme}$${count}$!!!$${hash}`,
      `${scheme}$${count}$${salt}$!!!`,
      `${scheme}$${count}$${b64(utf8("short"))}$${hash}`,
      `${scheme}$${count}$${salt}$${b64(utf8("tiny"))}`,
    ];
    for (const s of bad) {
      assert.equal(await c.verifyPassword(PW, s), false, JSON.stringify(s));
      assert.equal(await c.needsRehash(s), true, JSON.stringify(s));
    }
  });

  t("CryptoProvider.hashPassword: a hash is self-describing, salted, and verifies the same password only", async () => {
    const c = provider();
    const one = await c.hashPassword("correct horse");
    const two = await c.hashPassword("correct horse");
    assert.match(one, new RegExp(`^pbkdf2-sha256\\$${COUNT}\\$[A-Za-z0-9+/]{22}\\$[A-Za-z0-9+/]{43}$`));
    assert.notEqual(one, two, "a fresh salt each time");
    assert.equal(await c.verifyPassword("correct horse", one), true);
    assert.equal(await c.verifyPassword("correct horse", two), true);
    assert.equal(await c.verifyPassword("correct horsf", one), false);
    assert.equal(await c.verifyPassword("Correct horse", one), false);
    // The same thing `node:crypto` computes from the stored parameters.
    const [, count, salt, hash] = one.split("$");
    const theirs = nodeCrypto.pbkdf2Sync("correct horse", Buffer.from(salt, "base64"), Number(count), 32, "sha256");
    assert.equal(b64(theirs), hash);
  });

  t("CryptoProvider.hashPassword: the password is NFC-normalised, so composed and decomposed forms match", async () => {
    const c = provider();
    const composed = "café";
    const decomposed = "café";
    const stored1 = await c.hashPassword(composed);
    assert.equal(await c.verifyPassword(decomposed, stored1), true);
    assert.equal(await c.verifyPassword("cafe", stored1), false);
    assert.equal(await c.verifyPassword("", await c.hashPassword("")), true);
  });

  t("CryptoProvider.needsRehash: a lower count or a smaller salt or hash needs one; the current format does not", async () => {
    const c = provider();
    assert.equal(await c.needsRehash(await c.hashPassword("x")), false);
    assert.equal(await c.needsRehash(stored(4096)), true);
    assert.equal(await c.needsRehash(stored(COUNT)), false);
    assert.equal(await c.needsRehash(stored(COUNT - 1)), true);
    const fresh = (await c.hashPassword("x")).split("$");
    const shortSalt = `${fresh[0]}$${fresh[1]}$${b64(Buffer.from(fresh[2], "base64").subarray(0, 8))}$${fresh[3]}`;
    assert.equal(await c.needsRehash(shortSalt), true);
    const higher = `${fresh[0]}$700000$${fresh[2]}$${fresh[3]}`;
    assert.equal(await c.needsRehash(higher), false);
  });

  t("CryptoProvider.verifyPassword: a stored count the platform cannot run is false and says why, never a fault", async () => {
    const c = provider();
    const real = globalThis.crypto.subtle.deriveBits.bind(globalThis.crypto.subtle);
    // The limit deployed Workers have (the cloudflare binding documents it); a local run cannot see it, so
    // it is simulated here on every platform to pin what each binding does about it.
    globalThis.crypto.subtle.deriveBits = async (algorithm, key, length) => {
      if (algorithm.name === "PBKDF2" && algorithm.iterations > 100000) {
        throw Object.assign(new Error("iteration counts above 100000 are not supported"), { name: "NotSupportedError" });
      }
      return real(algorithm, key, length);
    };
    const warnings = [];
    const realWarn = console.warn;
    console.warn = (line) => warnings.push(String(line));
    try {
      assert.equal(await c.verifyPassword(PW, stored(150000)), false);
      assert.equal(warnings.length, 1);
      if (platform === "cloudflare") {
        // Refused before the platform is asked, naming the cap.
        assert.match(warnings[0], /150000 iterations: this platform's Web Crypto accepts at most 100000/);
      } else {
        // No cap is known here, so the platform is asked and its refusal is reported.
        assert.match(warnings[0], /refused PBKDF2 at 150000 iterations \(NotSupportedError\)/);
      }
      assert.ok(!warnings[0].includes(PW));
      assert.equal(await c.verifyPassword(PW, stored(4096)), true);
      if (platform === "cloudflare") {
        // This platform's own count is one the limit allows: it hashes, and verifies what it hashed.
        const own = await c.hashPassword("x");
        assert.match(own, /^pbkdf2-sha256\$100000\$/);
        assert.equal(await c.verifyPassword("x", own), true);
      } else {
        await assert.rejects(() => c.hashPassword("x"), /not supported/);
      }
    } finally {
      globalThis.crypto.subtle.deriveBits = real;
      console.warn = realWarn;
    }
  });
}

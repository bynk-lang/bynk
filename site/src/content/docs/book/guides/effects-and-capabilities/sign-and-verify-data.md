---
title: Sign and verify data
---
**Goal:** hash a token, check a webhook's HMAC by hand, and sign a payload with a
key held in `Secrets`, using the `Crypto` capability.

`bynk.crypto` is **experimental**: it carries no compatibility promise yet. Opt in
in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.crypto"]
```

Every operation is an `Effect`, because Web Crypto is asynchronous on every
platform. There is no hashing in pure functions, refinements or invariants, and
a handler that does cryptography says so in `given Crypto`. Encodings (hex,
base64, base64url) stay pure on `Bytes`.

## Store an API token as a digest

Keep only the SHA-256 of a token, and compare a presented token to it with
`Crypto.equal`, never `==`. `==` and `Bytes.compare` stop at the first differing
byte, so their timing reveals how much matched. `Crypto.equal` does not.

```bynk,fragment
context registry.publish

consumes bynk { Random }
consumes bynk.crypto { Crypto }

service issue {
  on call() -> Effect[String] given Random {
    let raw <- Random.bytes(32)
    Effect.pure(raw.toBase64Url())
  }
}

service digestOf {
  on call(token: String) -> Effect[String] given Crypto {
    let digest <- Crypto.digest(Sha256, Bytes.fromUtf8(token))
    Effect.pure(digest.toHex())
  }
}

service check {
  on call(presented: String, storedHex: String) -> Effect[Bool] given Crypto {
    match Bytes.fromHex(storedHex) {
      None => Effect.pure(false)
      Some(stored) => {
        let digest <- Crypto.digest(Sha256, Bytes.fromUtf8(presented))
        Crypto.equal(digest, stored)
      }
    }
  }
}
```

Use `Random.bytes` for anything secret. `Random.int` is not for secrets.

## Check a webhook's HMAC by hand

When the sender's signature scheme is not one the runtime verifies for you,
recompute the MAC over the exact bytes you received. `verifyHmac` compares in
constant time:

```bynk,fragment
context registry.hooks

consumes bynk { Secrets }
consumes bynk.crypto { Crypto }

service verify {
  on call(body: String, signatureHex: String) -> Effect[Bool] given Crypto, Secrets {
    let secret <- Secrets.get("WEBHOOK_SECRET")
    match secret {
      None => Effect.pure(false)
      Some(key) => match Bytes.fromHex(signatureHex) {
        None => Effect.pure(false)
        Some(mac) => Crypto.verifyHmac(Sha256, Bytes.fromUtf8(key), Bytes.fromUtf8(body), mac)
      }
    }
  }
}
```

## Sign a payload with a key from `Secrets`

Keep the private key in `Secrets` as base64 PKCS#8, import it where you sign, and
let it go out of scope. A `SigningKey` is an opaque handle to a non-extractable
platform key: it cannot be read back, stored, logged or sent. Using one in a
`store`, a record or event field, a handler parameter or result, or a wire type
is `bynk.types.key_at_boundary`.

```bynk,fragment
context registry.index

consumes bynk { Secrets }
consumes bynk.crypto { Crypto }

service signLine {
  on call(payload: String) -> Effect[Result[String, CryptoError]] given Crypto, Secrets {
    let stored <- Secrets.get("REGISTRY_SIGNING_KEY")
    match stored {
      None => Effect.pure(Err(InvalidKey("REGISTRY_SIGNING_KEY isn't set")))
      Some(b64) => match Bytes.fromBase64(b64) {
        None => Effect.pure(Err(InvalidKey("REGISTRY_SIGNING_KEY isn't base64")))
        Some(pkcs8) => {
          let imported <- Crypto.importSigningKey(Ed25519, pkcs8)
          match imported {
            Err(e) => Effect.pure(Err(e))
            Ok(key) => {
              let signature <- Crypto.sign(key, Bytes.fromUtf8(payload))
              Effect.pure(Ok(signature.toBase64()))
            }
          }
        }
      }
    }
  }
}
```

To verify, import the public key with `Crypto.importVerifyKey` (SPKI, or the 32
raw bytes of an Ed25519 key) or `Crypto.importVerifyJwk`, then call
`Crypto.verify`. A signature that is malformed is `false`, not a fault.

A key that cannot be imported is an `Err(CryptoError)` value. `InvalidKey` is a
key the platform could not parse for that algorithm, and `UnsupportedAlgorithm`
is an algorithm the platform lacks, such as Ed25519 in an older browser. Neither
carries key material. Both `CryptoError` and the algorithm enums are extensible,
so a `match` over them needs a wildcard.

## In a test

A unit-tier test runs the real cryptography. Digests, MACs and signatures are
deterministic for given inputs, so a fake would check nothing. A test sees `None`
from `Secrets` unless the case sets it in an `env { … }`. `stub Crypto.digest(_, _) returns …`
and `expect Crypto.digest called once` work as for any capability, and
`Random.bytes` gives the same bytes on every run.

## What you cannot do here

This unit has no encryption or decryption, key generation or export, key
derivation, JWT or JOSE helpers, or X.509. The runtime's Bearer and OIDC
verifiers are unchanged. In a browser, Web Crypto exists only in a secure context
(HTTPS or `localhost`); elsewhere an operation fails loudly rather than falling
back to something weaker.

**See also:** [`bynk.crypto` reference](/book/reference/bynk-capabilities/#cryptography--bynkcrypto),
[Verify webhooks](/book/guides/actors/verify-webhooks/).

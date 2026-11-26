---
title: Store a password
---
**Goal:** keep a user's password so that a stolen copy of your data does not give
the password away, check a login against it, and move old hashes to a stronger
setting as you raise it, using the `Crypto` capability.

`bynk.crypto` is **experimental**: it carries no compatibility promise yet. Opt in
in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.crypto"]
```

If you can avoid holding passwords, do: an identity provider, or `Oidc`, keeps
them for you. If you must, never store the password or a plain digest of it.
Store the output of `Crypto.hashPassword`.

## Hash a password when it is set

`hashPassword` returns a string to store. It is salted with a fresh random salt
each time, so two users with the same password have different hashes, and it
names its algorithm and iteration count, so you never store those separately:

```bynk,fragment
context accounts.passwords

consumes bynk.crypto { Crypto }

service hash {
	on call(password: String) -> Effect[String] by Caller given Crypto {
		Crypto.hashPassword(password)
	}
}
```

The result looks like `pbkdf2-sha256$600000$<salt>$<hash>`. Keep it as an
ordinary string field next to the user.

## Check a password at login

`verifyPassword` reads the parameters from the stored string, recomputes the
hash and compares in constant time. A wrong password is `false`. So is a stored
value that is not a hash in this format, which means a damaged row never logs
anybody in:

```bynk,fragment
context accounts.login

consumes bynk.crypto { Crypto }

service check {
	on call(password: String, stored: String) -> Effect[Bool] by Caller given Crypto {
		Crypto.verifyPassword(password, stored)
	}
}
```

## Raise the cost without locking anyone out

The iteration count is Bynk's to choose and rises over time as hardware gets
faster. Every stored hash records the count it was made with, so a release that
raises it still verifies the hashes you already have. After a successful login,
ask `needsRehash` and hash the password again, which is the one moment you hold
the password:

```bynk,fragment
context accounts.upgrade

consumes bynk.crypto { Crypto }

service login {
	on call(password: String, stored: String) -> Effect[Option[String]] by Caller given Crypto {
		let ok <- Crypto.verifyPassword(password, stored)
		if !ok {
			Effect.pure(None)
		} else {
			let stale <- Crypto.needsRehash(stored)
			if stale {
				let fresh <- Crypto.hashPassword(password)
				Effect.pure(Some(fresh))
			} else {
				Effect.pure(None)
			}
		}
	}
}
```

`Some(hash)` is a replacement to store; `None` means leave the row as it is, or,
for a failed login, that there is nothing to store.

## What it costs, and what Workers allows

A password hash is slow on purpose. It spends significant CPU, and that counts
against a request's CPU limit. **On Workers it needs the Paid plan.** Measured on
deployed Workers ([sandbox check](https://github.com/bynk-lang/bynk/issues/4118#issuecomment-7199935890)), a hash at the Workers count of 100,000
iterations costs about 30 ms of CPU per call (23 to 42 ms over five requests),
two to four times the Free plan's 10 ms per request. If you cannot use the Paid
plan, use an identity provider rather than a cheaper hash. There is no lower
setting, and that is deliberate.

The count differs by platform. Node and the browser use 600,000, OWASP's
guidance for this algorithm. Deployed Cloudflare Workers refuse more than
100,000 ([confirmed on a deployed Worker](https://github.com/bynk-lang/bynk/issues/4118#issuecomment-7198549618)), so there it is 100,000: about six times fewer rounds, and **below** that
guidance. If that is too weak for what you protect, hash elsewhere or use an
identity provider. A hash made off Workers at more than 100,000 cannot be
verified on Workers: `verifyPassword` returns `false` there and logs a warning
naming the count and the cap. Make hashes on the platform that will check them.

The password is normalised to Unicode NFC before hashing, so the same typed
password matches however a keyboard composed its characters. Anything beyond
that, such as a length limit or a breach check, is your policy and not the
hash's.

See [`bynk.crypto`](/reference/bynk-capabilities/#cryptography--bynkcrypto) for
every operation, and [Sign and verify data](/book/guides/effects-and-capabilities/sign-and-verify-data/)
for HMAC and signatures.

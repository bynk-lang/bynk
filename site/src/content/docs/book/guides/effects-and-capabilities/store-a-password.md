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

## What it costs

A password hash is slow on purpose. One at the current count can take on the
order of a second of CPU, and it counts against a request's CPU limit on
Workers. Measure it on your plan; if your plan's limit is too low for it, use an
identity provider rather than a cheaper hash. There is no lower setting, and
that is deliberate.

The password is normalised to Unicode NFC before hashing, so the same typed
password matches however a keyboard composed its characters. Anything beyond
that, such as a length limit or a breach check, is your policy and not the
hash's.

See [`bynk.crypto`](/reference/bynk-capabilities/#cryptography--bynkcrypto) for
every operation, and [Sign and verify data](/book/guides/effects-and-capabilities/sign-and-verify-data/)
for HMAC and signatures.

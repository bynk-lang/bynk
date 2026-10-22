---
title: Wrap a library as an adapter
---
You want to use an npm library (or a remote HTTP API) from Bynk. Wrap it in an
**adapter**: declare the capability contract in Bynk, implement it in a
TypeScript **binding**, and consume it like any other capability.

## 1. Declare the adapter

Name the adapter for the capability it provides. Declare the capability, any
boundary types, and an external (bodiless) `provides`. Name the binding module
and pin its npm dependency.

Configuration the binding needs — here the signing secret — is a **capability
dependency** (`consumes bynk { Secrets }` + `given Secrets`), not an operation
parameter (v0.18).

```bynk,fragment
adapter tokens {
  binding "./tokens.binding.ts" requires { "jose": "^5" }
  consumes bynk { Secrets }

  exports capability  { Jwt }
  exports transparent { Claims, JwtError }

  type Claims   = { sub: String, exp: Int }
  type JwtError = enum { Invalid, Expired }

  capability Jwt {
    fn sign(claims: Claims) -> Effect[String]
    fn verify(token: String) -> Effect[Result[Claims, JwtError]]
  }

  provides Jwt = JoseJwt given Secrets
}
```

If you publish this adapter in a package, each project that depends on it
through git or a registry approves `jose` at `^5` once, with `bynk approve`,
before its build installs it — see
[Approve a dependency's npm packages](/book/guides/projects-build-and-deployment/share-code-between-projects/#approve-a-dependencys-npm-packages).
Changing the range later asks every consumer again.

## 2. Write the binding

The binding lives beside the adapter source at the path the `binding` clause
names. `implements Jwt` against the generated interface is the contract — `tsc
--strict` enforces it. Construct boundary values **through the emitted
constructors** (`Ok`/`Err`, the sum type's `JwtError.Invalid`, a `Claims` object
literal) — never hand-rolled tag shapes.

The provider's `given` names arrive as a **by-name deps object** in the class
constructor — the keys are the `given` names, and `tsc` checks them.

```typescript
// tokens.binding.ts
import * as jose from "jose";
import type { Jwt, Claims } from "./tokens.js";
import { JwtError } from "./tokens.js";          // emitted variant constructors
import type { Secrets } from "./bynk.js";
import { Ok, Err, type Result } from "./runtime.js";

export class JoseJwt implements Jwt {
  constructor(private deps: { Secrets: Secrets }) {}

  async sign(claims: Claims): Promise<string> {
    return await new jose.SignJWT({ ...claims })
      .setProtectedHeader({ alg: "HS256" })
      .sign(new TextEncoder().encode(await this.secret()));
  }
  async verify(token: string): Promise<Result<Claims, JwtError>> {
    try {
      const { payload } = await jose.jwtVerify(
        token,
        new TextEncoder().encode(await this.secret()),
      );
      return Ok({ sub: String(payload.sub), exp: Number(payload.exp) });
    } catch {
      return Err(JwtError.Invalid);
    }
  }

  private async secret(): Promise<string> {
    const s = await this.deps.Secrets.get("JWT_SECRET");
    return s.tag === "Some" ? s.value : "";
  }
}
```

A **remote API** is the same shape with no npm dependency — drop the `requires`
clause and take `given bynk.Fetch` instead of calling the global `fetch`,
mapping the typed `Response` to a `Result`.

## 3. Consume it

```bynk,fragment
context auth.sessions {
  consumes tokens { Jwt }      // flatten `Jwt` into the local namespace

  service login {
    on call() -> Effect[String] given Jwt {
      let token <- Jwt.sign(Claims { sub: "u1", exp: 0 })
      token
    }
  }
}
```

Compile: the adapter's interface module and the binding are emitted into the
output, the npm dependency is folded into `package.json`, and the composition
root instantiates the binding's class and injects it. To swap the real
implementation in a test, `stub Jwt.verify(_) returns …` overrides that seam
(or promote the case to `as integration` to run the real one). To test the binding
itself, write a `suite` on the adapter: its cases call `Jwt.sign(…)` directly
against the real binding.

## See also

- [Adapters reference](/book/reference/adapters/)
- [Adapter & binding errors](/book/troubleshooting/adapter-errors/)

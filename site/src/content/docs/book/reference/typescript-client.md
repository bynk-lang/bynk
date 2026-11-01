---
title: TypeScript client
---
A Worker that isn't written in Bynk calls a context through a **typed
TypeScript client** the build generates. The client exposes one function per
service the Worker may call, with the context's boundary types as TypeScript
types. It signs each request, stamps the contract hash, and encodes the
arguments and decodes the result with the codec a Bynk caller uses. The
[wire format](/book/reference/wire-format/) underneath stays a detail behind
it.

For a walk-through, see
[Adopt Bynk beside a TypeScript Worker](/book/guides/projects-build-and-deployment/adopt-beside-a-typescript-worker/).

## When a client is emitted

A context gets a client when it declares at least one
[foreign caller](/book/reference/actors/) that one of its `on call` handlers
admits:

```bynk,ignore
actor Billing { auth = Foreign(secret = "BILLING_CALL_KEY", worker = "billing-svc") }

service lookup {
  on call(id: EntryId) -> Effect[Result[Entry, LedgerError]] by Billing { … }
}
```

The build writes it beside the Worker's `wrangler.toml`, as
`workers/<worker>/bynk-client.ts` (for `shop.ledger`,
`workers/shop-ledger/bynk-client.ts`). It covers only the services a foreign
caller may call; a handler that admits only Bynk contexts is left out. A
context with no foreign caller gets no client, because a Worker it hasn't
declared holds no secret and every call it made would be refused.

The client is written for the foreign Worker's build, not the Bynk one. The
build's own `tsconfig.json` excludes it, and a `--emit js` build strips it to
`bynk-client.js` like every other module, so take the typed client from a
TypeScript build (the default).

## What it exports

| Export | What it is |
|---|---|
| `connect(options)` | A client calling the context as `options.caller`. |
| `ConnectOptions<A>` | `{ caller: A, binding, env }`: the actor, the Service Binding to the context's Worker, and the Worker's `env`. |
| `Client<A>` | The services actor `A` may call, each a function returning a `Promise`. |
| `Services` | Every service in the client, typed. |
| `Admits` | The foreign callers each service admits. |
| `ForeignCaller` | The declared foreign callers' names, as a union of string literals. |
| `Env` | What the Worker's `env` must hold: the secret, by name. |
| `CONTEXT` | The context's qualified name. |
| `WIRE_VERSION` | The wire format version the client was generated for. |
| `SECRET` | The name the Worker holds its secret under, `BYNK_CALL_TO_<CONTEXT>`. |
| `CONTRACTS` | Each service's contract hash, as the callee was built. |

The boundary types are exported under their own names, with their codecs
(`__serialise_*`, `__deserialise_*`), which the client uses and you don't need
to call. A boundary type that shares a name with the client's own exports
(`Client`, `Services`, `Env` and the rest) is renamed with a trailing `_`
(`Client_`), so the client's surface is the same for every context.

`Client<A>` holds only the services `A`'s handlers admit. Calling another one
is a `tsc` error rather than a `403`:

```ts
const ledger = connect({ caller: "Ledger", binding: env.SHOP_ORDERS, env });
await ledger.status("o-1"); // admitted
await ledger.ping(1);       // error TS2339: Property 'ping' does not exist
```

## Types

Each boundary type is declared in its in-memory form, which is what the codecs
produce and take:

| Bynk | TypeScript |
|---|---|
| `Int`, `Float`, `Instant`, `Duration` | `number` |
| `String` | `string` |
| `Bool` | `boolean` |
| `Bytes` | `Uint8Array` |
| `Decimal` | `{ readonly coefficient: bigint; readonly scale: number }`, a JSON string on the wire (`"1.20"`); the client parses and writes it for you |
| `()` | `void` |
| `List[T]` | `readonly T[]` |
| `Map[K, V]` | `ReadonlyMap<K, V>` |
| `Option[T]` | `Option<T>`: `{ tag: "Some", value }` or `{ tag: "None" }` |
| `Result[T, E]` | `Result<T, E>`: `{ tag: "Ok", value }` or `{ tag: "Err", error }` |
| a record | an interface of `readonly` fields |
| a sum | a union discriminated by `tag`, each variant's payload fields beside it |
| a generic record `Page[T]` | a generic interface `Page<T>` |
| a refined type | its base type; the callee checks the predicate |
| an opaque type | its base type, branded, so you pass back a value you were given |

`Ok`, `Err`, `Some`, `None`, `Result` and `Option` come from
`@bynk/wire-client`. On the wire a variant is tagged `kind`, not `tag`; the
client converts between the two.

## Calling

`connect` takes the actor the Worker is declared as, its Service Binding to the
context's Worker, and its `env`:

```ts
import { connect, type EntryId, type Env } from "./bynk-client.js";
import { callErrorOf } from "@bynk/wire-client";

interface WorkerEnv extends Env {
  readonly LEDGER: Fetcher;
}

export default {
  async fetch(request: Request, env: WorkerEnv): Promise<Response> {
    const ledger = connect({ caller: "Billing", binding: env.LEDGER, env });
    try {
      const found = await ledger.lookup("E-1" as EntryId);
      return Response.json(found.tag === "Ok" ? found.value : { error: found.error.tag });
    } catch (e) {
      return new Response(callErrorOf(e)?.kind ?? "failed", { status: 502 });
    }
  },
};
```

`env` must hold the secret under the name in `SECRET`, typed by `Env`: a Worker
whose `env` lacks it fails `tsc`. `bynk deploy` mints the secret and prints the
`wrangler secret put` that sets it on the foreign Worker (see
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#foreign-callers)).
Locally, `bynk dev` gives the context the fixed key `bynk-local-call-key`; run
the foreign Worker with `--var BYNK_CALL_TO_<CONTEXT>:bynk-local-call-key`.

A service's own `Result` comes back as a value: a domain `Err` is an answer,
not a failure. A call that fails is **thrown**, and `callErrorOf(e)` reads the
error:

| `kind` | Means |
|---|---|
| `Transport` | The callee refused the request, or it did not arrive. `status` is the HTTP status: `401` (not signed under the right secret), `403` (the handler does not admit this caller), `400` (an argument broke a refinement the callee checks), `413` (the body is over the limit). |
| `ContractMismatch` | The callee was built at another contract (`409`); `expected` and `actual` name the two hashes. Both sides log it. |
| `CalleeFaulted` | The callee's handler faulted (`500`). `context` names the callee and `service` the service, so the cause is in that Worker's log; the error itself never crosses. Logged on the calling side as `CalleeFaulted <caller> -> <context> call <service>`. |
| `StructuralMismatch`, `RefinementViolation`, `MalformedJson` | The callee's answer did not decode. |

`callErrorOf` returns `undefined` for anything else.

## The runtime package

The client imports [`@bynk/wire-client`](https://github.com/bynk-lang/bynk/tree/main/bynk-emit/wire-client),
the runtime half: `callService`, the codec helpers, `Result`, `Option` and
`callErrorOf`. It isn't a copy of anything: it's built from the same runtime
modules every Bynk Worker embeds, so a foreign caller encodes, signs and
decodes as a Bynk caller does.

It follows the **wire format's** version, not the compiler's: its major version
is the wire version it speaks. A client states the version it was generated
for (`WIRE_VERSION: WireVersion`), so pairing it with a package for another
wire version fails `tsc`. The package ships TypeScript source, which the
Worker's bundler (`wrangler`, esbuild) compiles with the Worker's own code. Add
it to the foreign Worker's dependencies from the Bynk repository (a `file:` or
git dependency on `bynk-emit/wire-client`).

The client and the package build under `strict`, including
`noUnusedLocals` and `noUnusedParameters`. They don't build under
`noUncheckedIndexedAccess`.

## Skew

A client is generated at one time and the context deployed at another. Three
layers catch a contract that has moved under it:

1. **The caller's build.** A changed signature changes the client's types, so
   the Worker's code no longer type-checks once the client is regenerated. A
   change to the hash that leaves the TypeScript shape alone (a refined type's
   predicate) does not; to make `tsc` catch that too, pin the literal in your
   code: `const pinned: "586b853a77dcb029" = CONTRACTS.lookup;`.
2. **The context's deploy.** The context's project pins, per foreign caller,
   the hashes that caller's client was generated at. `bynk deploy` refuses a
   build that breaks a pin (below).
3. **The call.** A call stamped with another hash is refused with `409
   ContractMismatch`, and the callee logs it.

### Pins

The pins live in `bynk.foreign.toml`, beside `bynk.toml`, checked in. Each
table names a context and one of its foreign callers, and maps each service to
the hash that caller's client embeds:

```toml
["shop.ledger".Billing]
lookup = "586b853a77dcb029"
post = "27debbca3698ad27"
recent = "b8c0680d223be288"
```

The client's `CONTRACTS` doc carries these lines, ready to paste, and so does
`bynk deploy` for a foreign caller with no pin. `bynk deploy` holds each pin
against the build and **refuses**, before the plan and under `--dry-run` too,
when a pinned service's hash has changed, the caller may no longer call it, or
the context no longer declares the caller (or no handler admits it any more)
(`bynk.deploy.foreign_contract_skew`):

```text
bynk: this build breaks the contract a foreign caller's client was generated at (bynk.deploy.foreign_contract_skew):
  shop.ledger.lookup as `Billing` — pinned at 586b853a77dcb029, the build's is 0d1e6f7c2b3a4958
  Deploying this would ship a callee the foreign caller's client is refused by (409 ContractMismatch, or 403).
  Regenerate that caller's client (`bynk-client.ts`), then update its pin in bynk.foreign.toml; the new hashes are in the client's `CONTRACTS`.
```

Updating the pin is the override: it records that the foreign caller has been
regenerated. A service with no pin is reported, never refused, so the first
deploy of a new foreign caller goes through.

Each Worker's `bynk-contracts.json` records, under `foreign`, the hashes each
declared foreign caller may call it at, and the deploy record keeps them beside
what the Worker provides.

## Not native RPC

The client calls over the Worker's `fetch` and the published wire format, the
same path a Bynk caller takes, so the caller check and the codec stay in one
place. Cloudflare's native JavaScript RPC (`WorkerEntrypoint`) isn't emitted.

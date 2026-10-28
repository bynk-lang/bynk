---
title: Contract skew across independently deployed contexts
---
**Goal:** understand what happens when two contexts are deployed from different
versions of your source, why the compiler can see it, and what to do when a call
fails with `ContractMismatch`.

## The problem independent deployment creates

Contexts are independently deployable. That is the point — `shop.orders` and
`shop.payment` become separate Workers, and you can push one without the other.

It is also the hazard. When `orders` calls `payment`, the compiler type-checks
that call by reading `payment`'s source **at the time `orders` was built**. It
knows the parameter types, the return type, every field of every record that
crosses. It bakes that knowledge into `orders`.

Then time passes, and you deploy `payment` alone:

```bash
bynk deploy --context shop.payment
```

`payment` is now live with a contract that `orders` has never seen. `orders`
still believes the old one — and nothing in the two Workers knows they disagree.
The call still connects (a Service Binding resolves by name), the JSON still
parses, and the response is decoded by `orders` against a shape that is no longer
true. That is **contract skew**, and its natural failure mode is the worst kind:
not an error, but a wrong answer.

## What Bynk does about it

The compiler is the only party that ever sees both contracts, so it is the only
party that can record what it knew. Since v0.177 it does.

When it compiles `orders`, it computes a **contract hash** — a fingerprint of
`payment`'s contract as `orders` understands it — and stamps it into every call
site. When it compiles `payment`, it computes the same fingerprint from
`payment`'s own declarations and stamps that into `payment`'s Worker. Neither
Worker computes anything at runtime; both carry a constant fixed when they were
built.

At the boundary, `payment` compares the two before reading the request body. If
they differ, it refuses:

```json
{
  "kind": "ContractMismatch",
  "service": "authorise",
  "expected": "9f2c41ab73e05d18",
  "actual": "317bdd3de84d2176"
}
```

The caller sees a `ContractMismatch`, not a mangled result. The skew became a
loud, nameable failure instead of a silent one.

Both sides log it, so the name reaches your Worker logs and `wrangler tail`, not
just a response body passed between two Workers. `payment` logs it as it
refuses, and `orders` logs it before the call faults:

```text
ContractMismatch shop.payment call authorise { service: 'authorise', expected: '9f2c41ab73e05d18', actual: '317bdd3de84d2176' }
ContractMismatch shop.orders -> authorise { kind: 'ContractMismatch', service: 'authorise', … }
```

The fault itself answers `orders`'s own caller with a bare `500`, logged as
`shop.orders <route> faulted`, like any other fault.

### Why it refuses before reading the body

Once the two sides disagree about what the contract *is*, the body's meaning is
exactly what is in doubt. Validating it first would report something like
"expected string, got number" — true, but a description of the symptom. The
contract check runs first so the error names the actual fault.

## Better: `deploy` refuses first

A 409 in production is better than corruption, but it is still a failure your
users can see. `bynk deploy` catches it earlier.

Each Worker's build emits `bynk-contracts.json`, recording what the context
provides and what it expects of its dependencies. `deploy` remembers what it
pushed, so when you later run:

```bash
bynk deploy --context shop.orders
```

it compares `orders`'s expectations against what its dependencies actually have
live, and stops:

```
bynk: `shop-orders` was compiled against a contract its live dependencies no
      longer provide (bynk.deploy.contract_skew):
  shop.payment.authorise — compiled against 317bdd3de84d2176, live is 9f2c41ab73e05d18
  Deploying this would ship a caller its callee rejects (409 ContractMismatch) on every call.
  Deploy the whole project (`bynk deploy`) so both sides move together.
```

The same holds from the other side. `deploy` also records what each Worker
expects of its dependencies, so pushing the *callee* alone:

```bash
bynk deploy --context shop.payment
```

compares the contract `payment`'s new build provides against what every other
live Worker was compiled against, and stops before shipping a callee that would
refuse its live callers:

```
bynk: `shop-payment` no longer provides the contract its live callers were
      compiled against (bynk.deploy.contract_skew):
  `shop-orders` calls shop.payment.authorise — compiled against 317bdd3de84d2176, this build provides 9f2c41ab73e05d18
  Deploying this would ship a callee that rejects its live callers (409 ContractMismatch) on every call.
  Deploy the whole project (`bynk deploy`) so both sides move together.
```

Both checks refuse before the plan is printed, so `--dry-run` shows them too.
Under `--env staging`, the Workers are named `-staging` and the advice says
`bynk deploy --env staging`. A whole-project `bynk deploy` is never refused this
way, because it moves the callers with the callee.

A live caller can also be a Worker of a context you've since removed from the
project, still recorded in `bynk.deploy.lock` (the plan's `orphan worker`). It's
refused too, but deploying the whole project wouldn't help, because a deploy
never redeploys or deletes an orphaned Worker, and `--prune` doesn't either.
The refusal says so: if the Worker is unused, delete it with `wrangler delete`
and remove its entry from the lock file. If it's still needed, restore its
context and deploy the whole project.

A caller deployed by a driver older than this check has no record of what it
expects, so it can't be checked. `deploy` warns that it couldn't check that
caller, and the next whole-project deploy records it.

The runtime check stays regardless — it is the backstop for a `wrangler` push
that bypassed the driver, or a deploy record that has drifted. But in normal use
you should meet this at the command line, not in your logs.

## Compatible or breaking?

A hash says the contracts differ, not how. When `deploy` refuses a skew, it also
says whether the change is **compatible**, meaning the caller would still work if
the hashes agreed, or **breaking**, with the reasons:

```
bynk: `shop-payment` no longer provides the contract its live callers were
      compiled against (bynk.deploy.contract_skew):
  `shop-orders` calls shop.payment.authorise — compiled against 317bdd3de84d2176, this build provides 9f2c41ab73e05d18
      breaking response.refunded: the caller requires the field `refunded`, which the callee does not send (a field's default is not part of the contract, so only an `Option` may be absent)
      compatible response.receipt: the callee sends the field `receipt`, which the caller ignores
```

The rules are the wire format's own ([Wire format](/book/reference/wire-format/)).
A request flows from caller to callee and a response from callee to caller, and in
both the receiver must be able to decode everything the sender can produce:

| Change | In a request | In a response |
| ------ | ------------ | ------------- |
| Optional field added | compatible | compatible |
| Required field added | breaking | compatible |
| Field removed | compatible | breaking, unless it was an `Option` |
| Field made optional | compatible | breaking |
| Field made required | breaking | compatible |
| Field retyped | breaking | breaking |
| Refinement widened | compatible | breaking |
| Refinement narrowed or added | breaking | compatible |
| Sum variant added | compatible | breaking |
| Sum variant removed | breaking | compatible |

An unknown key is ignored and a missing `Option` is `None`, which is why most
rows come out as they do. A field's default is not part of the contract, so a
field added with a default still counts as required. A single parameter travels
as a bare value, so renaming it is compatible; several parameters travel as an
object keyed by name, and follow the field rules.

Whether a compatible change is allowed depends on which side moved. A
**callee** can be deployed ahead of its callers (next section). A **caller**
cannot be deployed ahead of its live callee, whatever the verdict: the callee
refuses a contract it was not built to accept. The verdict needs the structure
of the contract on record: `bynk.deploy.lock` stores it beside each hash, and a
ledger written by an older driver has hashes only, so those skews stay
unclassified until the next whole-project deploy fills them in.

The editor's contract-diff view reports the same verdict for a changed service,
from the same rules.

## Deploying a callee ahead of its callers

A change that is compatible for the callers can ship without a lock-step deploy:
roll the callee out first, then the callers at their own pace. Pass
`--accept-compatible-skew` with `--context`:

```bash
bynk deploy --context shop.payment --accept-compatible-skew
```

```
bynk: `shop-payment` will also answer 1 live caller contract that is compatible:
  `shop-orders` calls shop.payment.authorise — compiled against 317bdd3de84d2176
      compatible response.receipt: the callee sends the field `receipt`, which the caller ignores
  Recorded in bynk.contracts.lock: commit it. The callee answers these callers until they are redeployed; remove an entry once no live caller was compiled against it.
```

`deploy` takes the contract each live caller was compiled against from
`bynk.deploy.lock`, records it in **`bynk.contracts.lock`**, and rebuilds the
callee so it carries that contract's hash as well as its own. The file is
committed, like `bynk.schema.lock`, so the decision shows up in review:

```toml
version = 1

[accepts."shop.payment"]
authorise = ['Svc("authorise",P("id",B("String")),Ret(…))']
```

- **Each entry is checked against the contract the service has now**, on every
  build, never against the version before it. If an edit makes an older contract
  incompatible, the build warns (`bynk.project.contracts_lock_stale`), leaves the
  entry out, and the callee refuses those callers again.
- **A breaking change is never accepted**, with or without the flag, and neither
  is a caller ahead of its live callee: a callee refuses a hash it was never
  built to accept. The refusal says so, and names the callee to deploy first.
- **Only the older contracts listed are answered.** A hash the lock does not
  record is still `409 ContractMismatch`.
- **`--dry-run`** reports what a real run would record, and writes and builds
  nothing.
- **Pruning.** Once the callers are redeployed, nothing live needs the old
  entries. `deploy` says which ones, and you delete them from the lock. A
  service that accepts more than eight older contracts gets a warning
  (`bynk.project.contracts_lock_oversized`).
- **Not for a caller that isn't Bynk.** A foreign caller's pinned hash is held
  exactly (below).

A corrupt `bynk.contracts.lock` fails the build (`bynk.project.contracts_lock_corrupt`):
restore it from version control, because a callee that forgets the contracts it
accepts refuses its live callers.

## A caller that isn't Bynk

A Worker written in TypeScript has no `bynk-contracts.json` of its own, so
nothing above can see what it expects. It calls through a generated client
that embeds the hashes it was generated at, and the context's project pins
them in `bynk.foreign.toml`. `bynk deploy` refuses a build that breaks a pin
(`bynk.deploy.foreign_contract_skew`); regenerate that Worker's client, then
update the pin. See [TypeScript client](/book/reference/typescript-client/#skew).

## What to do when you hit it

**Deploy the whole project.** `bynk deploy` (no `--context`) pushes everything in
dependency order, so both sides move together and the hashes agree again. A
breaking contract change is a coordinated deploy; a compatible one can instead
go callee first (see above).

**If you did not think you changed a contract**, check what the hash actually
covers. It changes when the *wire* changes:

- a parameter renamed, retyped, added, removed, or reordered;
- a record field renamed, retyped, added, or removed;
- a sum variant added, removed, or its payload changed;
- a refinement predicate changed (`MaxLength(10)` → `MaxLength(11)`);
- the return type changed.

It deliberately does **not** change when nothing on the wire does:

- **reordering** a record's fields, or a sum's variants — a JSON object is
  unordered, and a sum carries a `kind` tag;
- **reordering** refinement predicates — `String where NonEmpty && MaxLength(10)`
  and `String where MaxLength(10) && NonEmpty` are the same type;
- reformatting, comments, or renaming anything the wire never carries;
- **tightening an `opaque` type's predicate.** A consumer cannot see inside an
  opaque type, so nothing it does can depend on that predicate — breaking it over
  a change it cannot observe would be noise.

If you get a mismatch you cannot explain from that list, that is a bug worth
reporting: a hash that fires when nothing changed is worse than no hash at all.

## The one-time rebuild

The first deploy after upgrading to v0.177 must rebuild **every** context. A
Worker built by an older compiler stamps no hash, and an absent hash fails closed
— deliberately, because a caller that predates the check is exactly the kind most
likely to be skewed. Run a full `bynk deploy` once and it settles.

## What this is not

It is **detection, not evolution**. Bynk tells you the two sides disagree; it does
not yet let them disagree safely by running two contract versions at once. And the
hash is not a security control — it is a compile-time constant on a trusted
internal channel, an accident detector, not a defence against a forged header.

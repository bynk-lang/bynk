---
title: Deploy to Cloudflare
---

Deploy a Bynk project to Cloudflare Workers with one command — every context,
in the right order:

```sh
bynk deploy
```

The first deploy checks Node and Wrangler, compiles the project, shows what it
will create, checks your Wrangler authentication, asks for confirmation, then
provisions each context's resources and deploys its Worker. Authenticate first
with `wrangler login` or `CLOUDFLARE_API_TOKEN`.

Use `--yes` in an automated, non-interactive invocation:

```sh
bynk deploy --yes
```

## Review a plan

No Cloudflare calls or project-state changes happen with `--dry-run` (or its
`--plan` alias):

```sh
bynk deploy --dry-run
bynk deploy --dry-run --format json
```

Arguments after `--` go straight to `wrangler deploy`, for example:

```sh
bynk deploy -- --minify
```

## From VS Code

The [VS Code extension](/docs/tooling/vscode-bynk/) deploys plan-first.
**Bynk: Deploy…** asks for an environment (the ones recorded in
`bynk.deploy.lock`, or a new one) and, in a multi-context project, whether to
deploy everything or one context. It then runs `bynk deploy --dry-run --format
json` and shows the plan: the upload order, what each context creates, reuses or
sets, the secrets it needs, and any [orphans](#orphans).

The plan's **Deploy** button runs the same `bynk deploy` in a terminal. The
extension never passes `--yes`, so `bynk` asks you to confirm there before it
changes anything. It never offers `--prune` either: orphans are listed with the
command to run yourself. Secrets stay out of the editor too. **Choose secrets
file…** passes a `--secrets-file` path for `bynk` to read, and a secret the plan
marks as required can also come from the environment or the terminal prompt.

**Bynk: Show Deploy Plan** shows the same plan with no Deploy button.

## What gets provisioned

`bynk deploy` creates exactly what your code already commits each context to —
nothing speculative. If it isn't in the generated `wrangler.toml`, it isn't
`bynk deploy`'s to create.

| In your context | What `bynk deploy` does |
|---|---|
| `consumes bynk.cloudflare { Kv }` | Creates a KV namespace and records its id |
| `capability Sessions = Kv` | Creates a KV namespace of its own for that instance, and records its id |
| `capability Archives = R2` | Creates an R2 bucket of its own for that instance, by name, and records it |
| `capability Main = Postgres` | Makes a Hyperdrive config from the connection string in the environment variable `[deploy.hyperdrive.Main]` names (or adopts a config you name by id), and records its id |
| `capability PublishLimit = RateLimit …` | Allocates the limiter a `namespace_id` and records it; nothing is created on the account |
| An `agent` | Nothing to create — the config declares its Durable Object class, and Cloudflare registers it |
| `service … from queue("n")` | Creates the queue `n` before pushing |
| `capability Events = Queue @name("n")` | Creates the queue `n` before pushing, if it isn't there |
| `capability Signups = Analytics` | Nothing to create — names the dataset; the platform makes it on the first write |
| An `on cron` handler | Nothing to create — the schedule rides the config |
| `consumes` another context | Nothing to create — it sets the deploy order |
| An `actor` with `auth = Bearer/Signature` | Sets that secret, and refuses to deploy without it |
| An `actor` with `auth = SignedCookie` | Mints its signing key (32 random bytes) when it is unset, and sets it; see [Session keys](#session-keys) |

A context using all three of the first kinds plans like this:

```
kv create ops-hub
queue create job-intake
durable object JobLedger (sqlite; advisory — Cloudflare reconciles it)
deploy ops-hub
```

### Queues

Queues are created **by name** — the name you wrote in `from queue("n")`. Before
each deploy, `bynk deploy` asks Cloudflare whether the queue is there and creates
it only if it isn't. So a queue you deleted outside Bynk comes back on the next
deploy, and a `reuse` line never means "and if it's gone, too bad".

Queues are created *before* the Worker is pushed, and this step is doing real
work: `wrangler deploy` does **not** create a queue for you — it checks, and
fails with `Queue "n" does not exist. To create it, run: wrangler queues create n`.
Creating them is what makes a queue-consuming context deployable in one command.

A queue a context sends to (`capability OrderEvents = Queue @name("n")`) is
created the same way. There is one difference: `bynk deploy` records a queue
that only producers name in `bynk.deploy.lock` **only if it created it**. A
queue that was already there may belong to a Worker outside the project, so it
never becomes an orphan that `--prune` could delete. Until it is recorded, the
plan shows such a queue as `queue create or reuse n`, since it can't tell which
the deploy will do. In a named environment the
producer's queue is qualified as a consumer's is (`n-staging`), so staging
never sends into the default environment's queue.

### Durable Objects

Each agent compiles to a Durable Object class, and so does the events fan-out
of a context whose handlers `emit`. The generated `wrangler.toml` declares every
one of them in Cloudflare's `exports` map:

```toml
[exports.JobLedger]
type = "durable-object"
storage = "sqlite"
```

There's no `[[migrations]]` block and no migration tag. On every deploy,
Cloudflare compares the classes your config declares with the Durable Object
namespaces the Worker already has, and creates whichever are missing. So adding
an agent to a context you've already deployed just works: the next
`bynk deploy` registers it. The same goes for the first `emit` in a context that
already has agents.

The plan's Durable Object lines are **advisory**, and worth understanding:

> **Understand — Cloudflare owns your Durable Object namespaces, not Bynk.** The
> plan lists the classes the push will *declare*, and never claims to know which
> namespaces already exist. `bynk.deploy.lock` deliberately keeps **no** record of
> them. A second record could disagree with the account (a class deleted outside
> Bynk, an account reset), and you'd be debugging Bynk's memory instead of your
> deployment.

Removing an agent is **loud**, twice over. `bynk deploy` refuses first, with
`bynk.agent.class_removed`, because the agent is still recorded in
`bynk.schema.lock`. If you take its entry out of the lock, Cloudflare refuses the
push instead, and the Worker you already deployed keeps serving:

```text
✘ [orphaned_provisioned_namespace] class 'Tally': class 'Tally' has a provisioned
  Durable Object namespace (…) but is neither in code nor in `exports`. This is
  an orphan; declare a `deleted` tombstone for the class to retire it.
```

Bynk never deletes or moves an agent's data unless you declare it:

- **`agent Basket replaces Cart`** emits a `renamed` tombstone
  (`[exports.Cart] state = "renamed" renamed_to = "Basket"`). Cloudflare moves
  `Cart`'s stored data to `Basket`. The plan shows
  `rename durable object Cart → Basket (moves its stored data)`.
- **`removed agent Wishlist`** emits a `deleted` tombstone. Cloudflare deletes the
  class and **all of its stored data, with no undo**. The plan shows
  `delete durable object shop.Wishlist (all stored data; irreversible)`.
  - **Confirming it.** `bynk deploy` won't push it until you confirm it by name:
    type the name at the prompt, or pass `--confirm-delete shop.Wishlist`
    (repeatable). `--yes` alone never deletes, and a `--confirm-delete` for a class
    nothing deletes is refused.
  - **Later deploys.** You confirm a deletion once. `bynk.deploy.lock` records the
    push that carried it, so a later deploy shows it as `already deleted by an
    earlier deploy` and asks nothing. A `--confirm-delete` left in a CI script for
    it is not refused.
- **A rename, once applied,** reads `already applied by an earlier deploy` in later
  plans, the same way: `bynk.deploy.lock` records the push that carried it.

Keep each declaration until a deploy prints Cloudflare's `stale_tombstone` notice
for it ("has no effect"), then delete it from your source. A renamed class gets a
new binding name (`BASKET`); if you hand-wrote a binding to the old one in your own
Wrangler config, update it.

**Rolling back.** Each agent's stored record carries the schema version of the
build that wrote it (`__v`). Redeploying an older build after a newer one has
written state makes those agents fault on load with `NewerSchema`, rather than
read a shape they don't know and write it back. Nothing is lost: deploy forward
again, or move the data by hand through your own handlers. See
[Change stored state safely](/book/guides/agents-and-state/stateful-agent/#change-stored-state-safely).

**Retiring migration steps.** [`bynk migrate sweep`](/docs/bynk-cli/#bynk-migrate-sweep)
wakes every stored agent instance in your deployed Workers, so the `migrate` steps
older instances needed can be deleted. For the length of a run, each Worker holds a
temporary secret, `BYNK_MIGRATE_KEY`, which signs the sweep's calls to its reserved
`/_bynk/migrate/` route. The sweep sets it first and deletes it when it finishes,
fails or is interrupted. Each set and delete creates a Worker version. Outside a run
the route answers `404`. If a sweep reports it couldn't delete the secret, delete it
yourself with the `wrangler secret delete` command it prints. After a sweep raises a
class's floor, roll back only to a build at or above it. See
[Retire old migration steps](/book/guides/agents-and-state/stateful-agent/#retire-old-migration-steps).

Each agent is a **SQLite-backed** Durable Object. That's the only backend the
Workers Free plan allows, and the one Cloudflare recommends on every plan. Your
agent code doesn't change, because a SQLite-backed Durable Object keeps the same
key-value storage API.

> **Breaking — Workers deployed by Bynk 0.309.10 or earlier.** Those versions
> created agents as **key-value-backed** Durable Objects. That only ever worked on
> the Workers Paid plan: the Free plan refuses a key-value-backed class
> (`code: 10097`), so a Free-plan account has no such Worker. Cloudflare can't
> change a class's backend in place, so redeploying such a Worker now fails. Delete the
> Worker (`wrangler delete`) and run `bynk deploy` again. **Deleting the Worker deletes its agents' stored state**,
> so if you need that state, read it out through your own handlers first and
> write it back after the redeploy. A Worker first deployed by 0.309.11 or later
> already has SQLite-backed agents and redeploys as it is: Cloudflare takes its
> classes over into `exports` with their state intact.
>
> This can't be undone either: once a Worker has deployed with `exports`,
> Cloudflare won't accept a `[[migrations]]` config for it again, so an older
> Bynk can't redeploy it.

## Secrets

`bynk deploy` sets your secrets before your code can serve a request, and
forgets them. A Worker that is already deployed gets them first, with
`wrangler secret put`, and then the push. A new Worker gets them in the same
upload as its code (`wrangler deploy --secrets-file`), so its first version
already has them. Values go from wherever you keep them straight to wrangler —
on stdin, or for a new Worker in a temporary file only you can read. That file
is deleted as soon as the upload ends, even if you press Ctrl-C or CI sends
`SIGTERM`: `bynk` waits for wrangler to stop before it exits. Only a `SIGKILL`
can leave the file behind, and the next `bynk deploy` deletes it. Values are
never written to `bynk.deploy.lock`, to generated config, or to the plan.

Each secret set prints its name, never its value:

```
bynk: set secret `AUTH_JWT_SECRET` on `api`
```

On a new Worker the line ends `, with its first version`. If your wrangler is
older than 4.74.0, which can't upload secrets with a new Worker, the deploy says
so and sets them just after the first push. For that moment the new Worker
answers `401` to any route that needs a secret.

Give secrets to `bynk deploy --secrets-file`, not to wrangler after `--`. A run
that includes a new Worker refuses `-- --secrets-file`, because that upload
already passes its own file.

Every line of the plan is marked with how Bynk came to know the name:

```
secret set AUTH_JWT_SECRET (declared)
secret set STRIPE_KEY (read)
secret set LEGACY_TOKEN (supplied)
deploy api
```

**Declared** — an actor's `auth` secret:

```bynk,fragment
actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }
```

The name is right there in your source, so `bynk deploy` reads it out of your
compiled project. You supply the value; you never name it.

**Read** — a `bynk.Secrets` lookup:

```bynk,fragment
let key <- Secrets.get("STRIPE_KEY")
```

Bynk sees this name too, as long as you pass a literal.

**Supplied** — anything you name yourself with `--secrets-file` or `--secret`,
that Bynk didn't find on its own.

The difference between `declared` and `read` is **not** how much Bynk knows about
them — it saw both names in your source. It's what happens when the value is
missing, and that follows from their types:

| | If you don't supply a value |
|---|---|
| `declared` — an actor's `auth` secret | **The deploy fails**, naming it. Unset, the Worker 401s every request. |
| `read` — a `Secrets.get("…")` name | **A warning.** `get` returns `Option`, so your code already handles `None` — Bynk won't refuse to ship a program that's correct. |

> **Understand — a `read` line is a warning, not a promise.** `Secrets.get` takes
> an ordinary `String`, so `Secrets.get(pickAName())` is legal Bynk and no
> compiler pass can say what it will ask for. When Bynk sees one it tells you at
> compile time:
>
> ```
> warning: bynk.secrets.computed_name
>   `Secrets.get` is called with a computed name, so `bynk deploy` cannot know
>   which secret this context reads
> ```
>
> …and the plan admits it:
>
> ```
> secrets incomplete api (reads at least one name the compiler cannot see)
> ```
>
> The same line appears when the Worker composes an adapter whose provider is
> `given Secrets`: the adapter's TypeScript binding can read any name, and the
> compiler cannot see into it. A literal `Secrets.get("X")` in a `provides`
> body is collected like one in a handler.
>
> **That line is the important one.** Without it, a short `read` list would be
> the most dangerous thing Bynk could show you: a list that's usually right gets
> trusted, and the one computed name it misses becomes a `None` in production
> that nobody thought to check for. So Bynk lists what it saw, and says plainly
> when that isn't everything.
>
> It would have been easy to forbid computed names instead — every `Secrets.get`
> in Bynk's own tree passes a literal, so the rule would cost nothing today. It
> isn't done because choosing a secret at runtime is a reasonable thing to want,
> and a language shouldn't take that away to make a deploy tool's list tidier.

### Supplying values

Names and values are separate questions. Names come from your actors (declared),
your `--secrets-file`'s keys, and each `--secret NAME`. Values are looked up per
name:

1. `--secrets-file` — a dotenv-style `NAME=value` file. Don't commit it.
2. The environment — checked for names Bynk already knows.
3. A prompt, if you're at a terminal.

```sh
bynk deploy --secrets-file .secrets.env          # names and values
STRIPE_KEY=sk_live_… bynk deploy --secret STRIPE_KEY   # value from the env
```

The environment is a **value** source only. `bynk deploy` never scans it for
names — deciding "these look like secrets" over your whole shell and uploading
them is not something a deploy tool should do to you.

In CI, where there is no terminal to prompt at, a missing value is a hard error
naming the secret. It is never a blank.

### Re-deploying

Cloudflare doesn't give secret values back, so `bynk deploy` can't compare yours
to what's up there — it can only see which names are **set**. So the default is
set-if-absent:

```
bynk: secret `AUTH_JWT_SECRET` is already set on `api`, skipping — use --force to overwrite
```

Pass `--force` when you've rotated a value. The default skips deliberately:
setting every secret on every deploy would cut a fresh Cloudflare secret version
each time, for nothing.

One consequence worth knowing: `--dry-run` never authenticates, which is what
lets you plan offline — so a plan says `set` for everything, and the skip shows
up when you actually run.

### Session keys

A [`SignedCookie`](/book/reference/actors/#signed-cookies) actor signs the session cookies it issues, so
it needs a key, and the key has to be long and random. You never choose one: when the variable named by
`secret` is unset, `bynk deploy` mints 32 random bytes (unpadded base64url), sets it on the Worker and
prints only its name, as for any secret. A plan lists it as `managed`.

- **Re-deploying keeps it.** A key the Worker already holds is left alone, so redeploying does not log
  anyone out.
- **`--force` replaces it, and ends every session signed under the old key.** Cloudflare never returns a
  secret's value, so `deploy` cannot keep a minted key as the `previous` one.
- **To rotate without a global logout,** supply the keys yourself (in `--secrets-file` or the
  environment): the new key as `secret`, and the old one, from your own record, as the actor's `previous`
  variable, which you set with `wrangler secret put`. Keep `previous` for one `ttl`, then remove it. A
  value you supply is checked here: it must be base64url of at least 32 bytes, so a weak key is refused
  at deploy rather than faulting the first login.
- **Contexts that share a key get one key.** A login service that issues the cookie and an API guarded
  by it list the same variable, and a cookie only verifies if both hold the same value. A first deploy
  mints one value and sets it on both. If one of them already holds a key and another does not, or a
  `--context` run reaches only some of them, `deploy` refuses and asks you to supply the value yourself
  (Cloudflare never returns a secret's value, so it cannot copy one across).
- **`bynk dev` uses a local key derived from the variable's name**, the same in every session (so two
  `bynk dev --context` sessions side by side accept each other's cookies) and different for each actor. It
  is for local development only; `deploy` never uses it. A `--var NAME:value` you pass after `--` is yours
  and is left alone.

### Edge keys

Each pair of contexts that talk to each other, a caller and the context it
calls or a publisher and a subscriber, shares a key of its own, which you never
write. The sender holds it as `BYNK_CALL_TO_<CALLEE>` and the receiver as
`BYNK_CALL_FROM_<CALLER>` (the context's name in capitals, a dot written `__`).
Every cross-context call and event delivery is signed with it, and a Worker
refuses one whose signature does not check out. Their paths (`/_bynk/call/…`,
`/_bynk/event/…`) answer on the Worker's public address too, so without the
signature anyone could call your `on call` services, claiming any context as the
caller. With it, a context can claim only its own name. The plan lists each key
on the context that holds it:

```
secret set BYNK_CALL_FROM_COMMERCE__ORDERS (managed)
deploy commerce-payment
secret set BYNK_CALL_TO_COMMERCE__PAYMENT (managed)
deploy commerce-orders
```

`bynk deploy` mints every key and never shows or stores the value. Both ends of
an edge must hold the same key, and Cloudflare won't give one back, so:

- A first deploy, a new edge, or an edge where either end lacks its key mints a
  fresh key and sets it on **both** ends. Calls along that edge can fail for the
  moment between one end taking the new key and the other.
- A redeploy where both ends hold the key leaves it alone. `--force` rotates the
  key of every edge the run deploys both ends of.
- `--context NAME` can't mint a key for an edge whose other end it doesn't
  deploy. If such an edge lacks its key, the deploy stops before pushing
  anything and asks for a whole-project deploy.

If a deploy that is rotating keys stops partway, the two ends of an edge can
hold different keys, and a plain rerun keeps both. The deploy names those edges
when it stops. Rerun `bynk deploy --force` to set one key on both ends.

You can't supply an edge key: one in `--secrets-file` is ignored, since spread
to every context it would break the edges it doesn't belong to. `bynk dev` and
`bynk test` use a fixed local key. A Worker you push with `wrangler deploy`
yourself needs its keys set too.

A key is a name the context is known by, so a context whose name can't be
written as one is refused at deploy: each dot-separated part must be lowercase
letters and digits, start with a letter, and use single underscores.

#### Foreign callers

A Worker that isn't Bynk can call a context's `on call` services once the context
declares it as a foreign caller:

```bynk,ignore
actor Ledger { auth = Foreign(secret = "LEDGER_CALL_KEY", worker = "ledger-svc") }

service status {
  on call(id: String) -> Effect[Result[String, String]] by who: Caller | Ledger { … }
}
```

The plan lists the secret on the context that declares it:

```
secret set LEDGER_CALL_KEY (managed)
```

The deploy mints the secret and sets it on your context. Your deploy never
writes to a Worker it doesn't own, so it then prints the command that hands
the secret to the other Worker's operator:

```
bynk: minted LEDGER_CALL_KEY for `Ledger`, a foreign caller of `shop.orders`. Its operator sets it on their Worker:

  printf '%s' '<value>' | npx wrangler secret put BYNK_CALL_TO_SHOP__ORDERS --name ledger-svc
```

The deploy prints the value only this once. It can't read it back later, so if
you lose it, run `bynk deploy --force` to mint and print a new one; the
foreign Worker then needs the new value. To share a value you already hold,
supply it (`--secrets-file`, or `LEDGER_CALL_KEY` in the environment). The
deploy then sets yours and prints nothing. Only your end is yours to set, so
`--context` can mint these too. The foreign Worker signs each call as `Ledger`,
as the [wire format](/book/reference/wire-format/#foreign-callers) describes.
`bynk dev` gives the secret a fixed local value.

A TypeScript Worker calls through the typed client the build generates for the
context, `bynk-client.ts` ([TypeScript client](/book/reference/typescript-client/)).
Pin the contract that client was generated at in `bynk.foreign.toml`, beside
`bynk.toml`, and the deploy refuses a build that would break it
(`bynk.deploy.foreign_contract_skew`) until you regenerate the client and update
the pin. A foreign caller with no pin is reported, with the lines to add:

```
bynk: 1 foreign caller has no pin in bynk.foreign.toml, so a contract change that breaks its client would not be refused. To pin, add:

  ["shop.orders".Ledger]
  status = "226b9939f85e69ea"
```

#### Upgrading from the shared call secret

A project deployed before edge keys used one secret, `BYNK_CALL_SECRET`, on
every Worker. Workers built now no longer read it, and the new ones refuse
calls from Workers still on it, and the other way round. So:

1. Redeploy the **whole project** once (`bynk deploy`, not `--context`). Every
   edge lacks its key, so the deploy mints and sets them all. Calls between a
   Worker already pushed and one not yet pushed fail until the run finishes.
2. Optionally delete the old secret from each Worker, which nothing reads now:
   `wrangler secret delete BYNK_CALL_SECRET --name <worker>` for each Worker
   (the context's name with its dots as dashes, as the plan shows it).

### Several KV namespaces

Each [derived capability](/book/reference/capabilities/#derived-capabilities)
(`capability Sessions = Kv`) is a namespace of its own. The plan gives it a line
of its own, with its binding:

```
kv create shop-cart
kv create shop-cart-price-cache (KV_PRICE_CACHE)
kv create shop-cart-sessions (KV_SESSIONS)
deploy shop-cart
```

The first line is the bare `Kv`'s namespace, titled with the Worker's name. It
appears only if the context uses the bare `Kv`. An instance's namespace is
titled `<worker>-<capability>`. Both get an environment suffix under `--env`
(`shop-cart-staging`, `shop-cart-sessions-staging`). Each id is
recorded in `bynk.deploy.lock` under its Worker and its binding, and only ever
written into the generated `wrangler.toml` immediately before the push.

Titles are account-wide, so two of a project's namespaces must not share one:
Worker `shop` with `capability CartSessions` and Worker `shop-cart` with
`capability Sessions` would both be `shop-cart-sessions`. `bynk deploy` refuses
that before planning, `--dry-run` included, and names both namespaces.

### R2 buckets

Each [`R2` instance](/book/reference/bynk-capabilities/#object-storage-r2)
(`capability Archives = R2`) is a bucket of its own. There is no id to mint: the
name is the identity, so the plan says `create` until the ledger records the
bucket and `reuse` after:

```
r2 create shop-cart-archives (R2_ARCHIVES)
r2 create shop-cart-indexes (R2_INDEXES) in jurisdiction eu
deploy shop-cart
```

A bucket is named `<worker>-<capability>`, with an environment suffix under
`--env` (`shop-cart-archives-staging`), and created with
`wrangler r2 bucket create`, in its `@jurisdiction` when it has one. R2 allows
3 to 63 lowercase letters, digits and hyphens, not starting or ending with a
hyphen. A derived name outside that is refused at plan time, naming the
instance, as is a name two instances would share. R2 bucket names are treated
as account-wide, so a name another environment's bucket already holds is also
refused. The ledger records each bucket's name and jurisdiction under its Worker
and binding, in `r2_instances`.

Removing or renaming the capability leaves an orphan, reported as
`orphan r2 shop-cart R2_ARCHIVES (shop-cart-archives; not deleted)`.
`--prune` never deletes a bucket: it holds data, and R2 refuses to delete a
non-empty one. Delete it yourself, then remove its entry from
`bynk.deploy.lock`. Changing `@jurisdiction` cannot move a bucket: the plan
says `recreate`, creates a new bucket, and leaves the old one and its objects
where they are, so deploy refuses without `--yes` until you have a plan for the
data. The old bucket stays in the ledger and is reported as an orphan
(`orphan r2 shop-cart R2_INDEXES (shop-cart-indexes, jurisdiction fedramp; not deleted)`)
until you remove its entry. `bynk dev -- --remote` binds the recorded buckets and refuses one no
deploy has created.

### Rate limiters

Each [`RateLimit` instance](/book/reference/bynk-capabilities/#rate-limiting-ratelimit)
(`capability PublishLimit = RateLimit @limit(20) @period(60.seconds)`) needs a
`namespace_id`: a number, unique on the Cloudflare account, that names its
counters. Cloudflare doesn't mint one; you choose it, so `bynk deploy` allocates
it in the ledger and nothing is created on the account:

```
ratelimit allocate RATELIMIT_PUBLISH_LIMIT
deploy shop-cart
```

The first deploy derives a starting number from the environment, the Worker and
the binding, takes the first number from there up that no limiter in any
environment of `bynk.deploy.lock` holds (environments may share an account), and
records it, in `ratelimit_instances`, before the push. Starting from the Worker's
name keeps two Bynk projects on one account apart: each has its own ledger and
can't see the other's ids, but a Worker's name is unique on the account, so their
limiters start in different places. They would only meet by a hash collision in
a range of a billion. Later deploys reuse it
(`ratelimit reuse RATELIMIT_PUBLISH_LIMIT (namespace 1000)`), and under `--env`
the environment gets its own. In CI an unrecorded limiter is refused, as an
unrecorded KV namespace is: deploy locally first and commit the ledger, or a CI
run would allocate an id it can't keep and reset the counters next time.

Changing `@limit` or `@period` is an ordinary redeploy that keeps the id: they
are the binding's configuration, not an account resource. Removing the
capability forgets its row once the push succeeds. Nothing can tell whether a
Worker that `bynk deploy` doesn't manage, on the same account, already uses a
number; if one does, the two would share counters. Hand-chosen ids are usually
small, and allocated ones rarely are.

### Analytics Engine datasets

An `Analytics` instance (`capability Signups = Analytics`) writes to a dataset
named for its Worker and the capability, in snake case: `shop.cart`'s `Signups`
is `shop_cart_signups`. In a named environment the name gains `_<env>`, with
`-` as `_` (`shop_cart_signups_staging`), so staging never writes into the
default environment's dataset.

There is nothing to create: the platform makes a dataset on its first write,
and it has no id. So the ledger records nothing, the plan names the dataset
without an action (`dataset shop_cart_signups (ANALYTICS_SIGNUPS; created on
first write)`), and `--prune` never deletes one. If a dataset with the derived
name is already on the account, the Worker writes into it.

Two instances whose names meet are refused before the plan, since their events
would mix for good: `shop.cart`'s `Signups` and `shop`'s `CartSignups` are both
`shop_cart_signups`, and `shop`'s `SignupsStaging` is `shop`'s `Signups` under
`--env staging` (checked once the project has deployed to `staging`). Rename one
capability.

### D1 databases

Each [`D1` instance](/book/reference/bynk-capabilities/#sql-databases-d1)
(`capability Orders = D1`) is a database of its own, named `<worker>-<capability>`
with an environment suffix under `--env` (`shop-cart-orders-staging`).
Cloudflare mints its id, so the plan says `create` until the ledger records the
database and `reuse` after:

```
d1 create shop-cart-orders (D1_ORDERS)
deploy shop-cart
```

The first deploy runs `wrangler d1 create`, records the name and the
`database_id` Cloudflare minted, in `d1_instances`, before the push (an
interrupted run never makes a second one), and writes both into the generated
`[[d1_databases]]` entry. If a database of that name is already on the account,
deploy adopts it, looking its id up with `wrangler d1 list`. Under `--env` the
environment gets its own database and id.

Removing or renaming the capability leaves an orphan, reported as
`orphan d1 shop-cart D1_ORDERS (shop-cart-orders; not deleted)`. `--prune`
never deletes a database: it holds data. Delete it yourself, then remove its
entry from `bynk.deploy.lock`. `bynk dev -- --remote` binds the recorded
database and refuses one no deploy has created; plain `bynk dev` uses a local
database of wrangler's.

A database's schema comes from its
[migrations](/book/reference/bynk-capabilities/#schema-migrations),
`migrations/<database>/NNNN_<name>.sql` in the project. The plan lists the
files of each database that has any. It runs offline, so it can't say which
of them the database has already applied:

```
d1 reuse shop-cart-orders (D1_ORDERS)
d1 migrate shop-cart-orders (2 files; pending ones applied before the push)
deploy shop-cart
```

Deploy runs `wrangler d1 migrations apply D1_ORDERS --remote`, with `--env`
under one, after the database is bound and before the push, so new code never
meets an old schema. Wrangler applies each file the database hasn't recorded,
each in its own transaction, and records it in the database's
`d1_migrations` table. The ledger records nothing about migrations. If a
migration fails, wrangler rolls that file back, the earlier ones stay
applied, and deploy stops with wrangler's message before anything is pushed,
so the previous code keeps serving. Deploy's own confirmation covers the
apply, so wrangler doesn't ask again.

### Postgres databases: Hyperdrive

Each [`Postgres` instance](/book/reference/bynk-capabilities/#postgres-databases-postgres)
(`capability Main = Postgres`) binds a Hyperdrive config, and `bynk deploy` makes
it. Tell it where the database's connection string is, in `bynk.toml`, by the
**name of an environment variable**:

```toml
[deploy.hyperdrive.Main]
connection_env = "MAIN_DATABASE_URL"
```

```sh
export MAIN_DATABASE_URL="postgres://user:password@db.example.com:5432/shop"
export CLOUDFLARE_API_TOKEN=…   # needs "Hyperdrive: Edit", and CLOUDFLARE_ACCOUNT_ID
bynk deploy
```

The key is the capability's name (`Main`). The string is a secret, so `bynk.toml`
never holds it, and a connection string written there is refused. Deploy reads the
variable, makes the config through Cloudflare's API (the account's
`hyperdrive/configs`, which takes the string as its parts, so
`postgres://user:password@host:port/database` must be a Postgres URL; a query
string such as `?sslmode=require` is ignored), records its id in the ledger and
writes the id into the generated `[[hyperdrive]]` entry, which a named environment
(`--env staging`) copies. The config is named `<worker>-<capability>`, with the
environment suffix under `--env` (`shop-orders-main-staging`), and each
environment makes its own:

```
hyperdrive create shop-orders-main (HYPERDRIVE_MAIN; from `MAIN_DATABASE_URL`)
deploy shop-orders
```

**Every deploy syncs the config.** When `connection_env` is set, deploy updates
the config from the variable each time, so rotating the password or moving the
database needs nothing but the new value in the variable. The update is
idempotent and keeps the config's id. The ledger records only the id and the name:
the ledger is committed, so nothing derived from the string, not even a digest,
is kept there. The plan says where the config is synced from, and never shows the
value:

```
hyperdrive sync shop-orders-main (HYPERDRIVE_MAIN; from `MAIN_DATABASE_URL`)
deploy shop-orders
```

**A missing variable fails the deploy**, `--dry-run` included, before the plan,
naming both:

```
bynk: the environment variable `MAIN_DATABASE_URL` is not set, and the `Postgres` capability behind `HYPERDRIVE_MAIN` of `shop-orders` reads its connection string from it (set it, or give environment `default` a `config_id` in `[deploy.hyperdrive]` to adopt a config that exists)
```

**Adopting a config that exists.** A team whose Hyperdrive config already exists
does not need its connection string at deploy. Name the config's id per environment
and nothing else is read:

```toml
[deploy.hyperdrive.Main]
connection_env = "MAIN_DATABASE_URL"          # used by environments with no config_id
config_id = { production = "0123456789abcdef0123456789abcdef" }
```

```
hyperdrive adopt 0123456789abcdef0123456789abcdef (HYPERDRIVE_MAIN)
```

Deploy checks the config is on the account, records it as adopted and binds it. It
never updates or deletes an adopted config. Use this too when you deploy from CI:
a config id minted in a CI run that cannot commit `bynk.deploy.lock` would be lost,
so CI refuses to make one (it names the binding and the two ways out).

Renaming the capability makes a new config and orphans the old one:

```
orphan hyperdrive shop-orders HYPERDRIVE_MAIN (shop-orders-main; deleted by --prune)
hyperdrive create shop-orders-primary (HYPERDRIVE_PRIMARY; from `MAIN_DATABASE_URL`)
```

A Hyperdrive config holds no data, so `--prune` deletes an orphan the deploy made
(an adopted one is only ever listed). `bynk dev -- --remote` binds the recorded
config and refuses one no deploy has made; plain `bynk dev` is
[configured separately](/book/guides/projects-build-and-deployment/run-locally/#a-local-postgres-database).

The connection string goes to Cloudflare over TLS and nowhere else. It is not in
the ledger, the plan or its JSON, `wrangler.toml`, `out/`, a log or an error
message: what Cloudflare answers is scrubbed of it before it is shown.

#### MySQL databases

A [`MySql` instance](/book/reference/bynk-capabilities/#mysql-databases-mysql)
(`capability Legacy = MySql`) deploys exactly as above, with a `mysql://` URL
(`mysql://user:password@db.example.com:3306/shop`, the port defaulting to 3306)
under `[deploy.hyperdrive.Legacy]`. The build writes `bynk-hyperdrive.json` beside
the Worker's `wrangler.toml`, which is how deploy knows the config is MySQL's and
what `@tls` the instance asked for. A connection string for the other database
(`postgres://` for a `MySql`) is refused, naming the scheme it wants.

`@tls("required")`, the default, is sent as the config's `sslmode` (`REQUIRED`).
`@tls("verify-ca")` and `@tls("verify-identity")` need a CA certificate your
account holds, which deploy does not upload, so they cannot be made from a
connection string: deploy stops, and names the commands that upload the CA and make
the config by hand, which you then adopt with `config_id`.

### Account secrets: Secrets Store

A `StoredSecret` instance
(`capability StripeKey = StoredSecret @store("payments") @secret("stripe-live")`)
reads a secret of a Cloudflare Secrets Store. The store and the secret belong
to the account, so deploy **links** them and never creates either. Before
anything is pushed, `bynk deploy` asks the account for the store named in
`@store`, checks that the secret named in `@secret` is in it, and records the
store's id in `bynk.deploy.lock`; a missing store or secret fails the deploy,
naming both, with the command that creates the secret:

```
bynk: `StripeKey` of `shop-billing` reads secret `stripe-live` of Secrets Store `payments`, which does not exist: create it with `wrangler secrets-store secret create <store-id> --name stripe-live --scopes workers --remote`
```

The plan shows the binding as linked, with no action:

```
stored secret SECRET_STRIPE_KEY stripe-live in store payments (linked; not created)
```

`--dry-run` stays offline and does not ask. The lookup uses Cloudflare's API,
so it needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, the token with
the "Secrets Store: Read" permission; a refusal says so. The store's name is used
as declared, without an environment suffix, and `--prune` never touches one.
The secret's value never reaches `bynk`, the plan, the ledger or generated
config: only its name and the store's id do.

**Per-Worker secrets or stored secrets?** [`bynk.Secrets`](/book/reference/bynk-capabilities/#the-portable-surface--bynk)
reads a secret set on one Worker, the portable kind that `bynk deploy` sets from
your environment (see [Secrets](#secrets)). A stored secret is account-wide:
shared across Workers, rotated once in the store, and Cloudflare-only. Use
`bynk.Secrets` unless several Workers must share one value or you rotate it
centrally.

### Sender domains: Email Sending

An `Email` instance with `@allowedSenders`
(`capability Mail = Email @allowedSenders(["orders@shop.example"])`) may send
only as those addresses, and Cloudflare sends only from a domain onboarded to
Email Sending. Onboarding is DNS and a dashboard step, so deploy **checks** it
and never configures it. Before anything is pushed, `bynk deploy` finds each
distinct sender domain's zone on the account and checks Email Sending is enabled
on the domain (or on a wildcard covering it); every domain that is not fails the
deploy, together, naming the domain:

```
bynk: `EMAIL_MAIL` of `shop-orders` may send as `orders@shop.example`: `shop.example` does not have Email Sending enabled on this account; onboard it under Email Service > Email Sending in the dashboard (`bynk deploy` checks a sender domain and never configures one)
```

`--dry-run` stays offline and does not ask. The lookup calls Cloudflare's API, so
it needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, the token able to
read the zone and its Email Sending settings; a refusal says so. Cloudflare's API
reports whether Email Sending is *enabled* on a domain, not whether its DNS
records have propagated, so a domain still propagating can pass the check and be
refused at run time as `SenderNotVerified`, which a handler can answer. An
instance without `@allowedSenders` asks nothing.

### Receiving mail: Email Routing

A context with a [`from email`](/book/guides/entry-points/receive-email/) service
gets its addresses in the Worker's `wrangler.toml`, as wrangler's top-level
`addresses`, lower-cased and sorted:

```toml
addresses = ["*@shop.example", "support@shop.example"]
```

`wrangler deploy` (4.113.0 or later; Bynk requires 4.120.1) reconciles the
account's Email Routing rules to that list, so an address a service claims has a
rule that delivers to the Worker, and one that is no longer listed loses its rule.
The list applies to every environment, so it is top-level and never copied into
an `[env.*]` table. A Worker with no email service has no `addresses` key, and
wrangler manages no routing rule for it. Two services in the project claiming one
address, even in different contexts, is a compile error
(`bynk.email.duplicate_address`), because Email Routing keeps one rule per address.

The plan lists each address as an advisory line (`email address
support@shop.example (Email Routing rule; advisory — wrangler reconciles it)`),
and `--format json` carries them as the context's `email_addresses`: wrangler,
not Bynk, owns the rules.

Wrangler deletes the rule of an address a Worker no longer lists, and mail to it
then bounces, so a removal is gated. Each push records the addresses it carried
in `bynk.deploy.lock`; the next plan shows an address that has gone as
`email address help@shop.example removed (…; needs --prune)` (and as
`removed_email_addresses` in JSON), and the deploy is **refused before anything is
pushed** unless `--prune` is given, with its usual confirmation (`--yes` answers
it; `--yes` alone does not). A first deploy has nothing recorded and asks
nothing, and neither does an added address. Dropping every `from email` service
removes every recorded address the same way.

Email Routing must be turned on for the domain; deploy does not check that.

#### Forward destinations

A `Forward(to)` destination must be a **verified destination address** in the
account, or Cloudflare refuses the forward: the handler is answered
`Reject("forward refused")`, the sender's mail bounces, and the cause shows only in
the Worker's logs. So deploy resolves each *literal* destination, a string written
in the handler (`Forward("finance@shop.example")`), against the account's Email
Routing destination addresses and **warns** when one is missing or not yet
verified. It is a warning, not a failure: verifying is a human step, and a failing
deploy would block unrelated changes until someone had clicked a link.

```text
bynk: warning: `billing` (context `shop.support`) forwards to `finance@shop.example`, which is not verified yet. Cloudflare refuses a forward to an address that is not verified, so the sender's mail bounces ("forward refused"). The owner of `finance@shop.example` verifies it by clicking the link Cloudflare emailed to that address; its status is under Email Routing → Destination addresses.
```

To verify an address, open **Email Routing → Destination addresses** in the
dashboard and add it if it is not listed; Cloudflare emails a link to that address,
and its owner clicks the link. Deploy finds the literals from the checked program,
so a comment or an unrelated string that spells an address counts for nothing. A `Forward` in a helper `fn` of the context is attributed to the context's email handlers. A
destination computed at run time (`Forward(lookup(mail))`) cannot be known at
deploy, so the plan says so once per handler that does it and leaves the check to
you. Addresses are matched with the domain case-insensitive and the local part
exactly as Cloudflare stored it; a destination that differs only in the local part's
letter case is reported as missing, naming the stored spelling.

The lookup calls Cloudflare's API (`GET /accounts/{account_id}/email/routing/addresses`,
every page), so it needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, the
token with the account-level **Email Routing Addresses: Read** permission (Write
also works). It is needed only for a project with an inbound email Worker that
forwards to a literal address. Without the permission, or without the credentials,
deploy prints one warning saying the check was skipped and carries on. `--dry-run`
shows the same warnings.

#### Dependencies

The Worker needs `postal-mime` installed beside it: the build's `package.json`
lists it, exactly pinned. `bynk deploy` (and `bynk dev`) install any listed
package that is missing into `.bynk/deploy`, before wrangler runs, with the
package manager you already use (pnpm, bun, Yarn classic or npm); a warm re-run
installs nothing and needs no network. `--no-install` (or `BYNK_NO_INSTALL=1`)
stops instead, naming the command to run, for CI that provisions `node_modules`
itself. `--dry-run` never installs: it reports what it would install, and with
which tool. See [Installing npm packages](/docs/bynk-cli/#installing-npm-packages).

### Workers outside the project

A [`Service` instance](/book/guides/effects-and-capabilities/call-a-foreign-worker/)
(`capability Billing = Service @worker("legacy-billing")`) binds a Worker that
is not Bynk's. Deploy never creates or changes it, so the plan shows the binding
with no action:

```
service SERVICE_BILLING legacy-billing (foreign Worker; not provisioned)
deploy shop-checkout
```

After authenticating, and before asking to confirm, `bynk deploy` asks the
account whether each such Worker exists (`wrangler deployments list --name`)
and warns, never refuses, when one does not. Cloudflare rejects an upload whose
Service Binding names a missing Worker, so deploy that Worker first.
`--dry-run` stays offline and does not ask. The Worker is not ordered against
the project's own, and under `--env` its name is used as declared, without an
environment suffix.

## Multi-context projects

A project with several contexts ships every one of them, in the order their
Service Bindings require. The plan shows that order before anything is touched:

```
kv create commerce-payment
deploy commerce-payment
deploy commerce-orders
order commerce-payment → commerce-orders
```

`commerce-payment` goes first because `commerce-orders` binds to it.

> **Understand — why the order is not just tidiness.** Cloudflare resolves a
> Service Binding when the Worker is **uploaded**, not when a request arrives.
> Deploying `commerce-orders` before `commerce-payment` exists doesn't merely
> leave a window where calls fail — the *upload itself is rejected*. So the
> order is a correctness requirement, and `bynk deploy` derives it from your
> `consumes` declarations rather than asking you to remember it.
>
> You never have to untangle a cycle to make this work: `consumes` cycles are
> already a compile error (`bynk.context.consumes_cycle`), so a project that
> compiles always has an order.

### Deploying one context

`--context` re-pushes a single context, which is what you want when iterating on
one service in a topology that is already live:

```sh
bynk deploy --context commerce.orders
```

It does **not** deploy that context's dependencies. If it binds to a context that
has never been deployed, `bynk deploy` says so and stops, rather than sending an
upload Cloudflare would reject:

```
bynk: `commerce-orders` binds to `commerce-payment`, which has never been
deployed — a Service Binding to a Worker that does not exist fails at upload.
  Deploy the whole project once (`bynk deploy`) to bring the topology up.
```

Bring a fresh topology up with a whole-project `bynk deploy` first; use
`--context` for iteration after that.

`--context` also refuses a push that would leave two live Workers disagreeing
about a contract (`bynk.deploy.contract_skew`). It refuses a context compiled
against a contract its live dependencies no longer provide. It also refuses a
context whose new build no longer provides what a live caller was compiled
against, naming each caller, the service and both hashes. Either way, deploy
the whole project so both sides move together. See
[Contract skew](/book/guides/projects-build-and-deployment/contract-skew/#better-deploy-refuses-first).

### When a deploy fails part-way

A multi-context deploy is **resumable, not transactional**. If the third context
fails after two succeeded, the two that landed stay deployed and recorded — there
is no automatic rollback — and the run stops rather than pushing on into a topology that
isn't what the plan described. Fix the problem and re-run `bynk deploy`: it
re-pushes in the same order, reusing the KV namespaces it already created. To go
back instead, see [Roll back a deployment](/book/guides/projects-build-and-deployment/roll-back-a-deployment/).

A half-deployed project is a real state, not a corrupt one, and the next plan
describes it honestly — contexts already live read `redeploy` rather than
`deploy`.

## Environments

By default `bynk deploy` ships to a single, unqualified environment. Pass
`--env` to ship the same project to more than one — `staging` and `production`
provision, record, and reconcile entirely independently, even on the same
Cloudflare account:

```sh
bynk deploy --env staging
bynk deploy --env production
```

Omit `--env` and nothing changes from before — it is the same as
`--env default`.

> **Understand — why `bynk deploy` writes more than `--env` to Wrangler.**
> Cloudflare does not carry your bindings into a named environment
> automatically: KV namespaces, queues, and Service Bindings all have to be
> declared again under that environment, or Wrangler deploys with none of
> them. `bynk deploy` does this for you — it writes an environment-scoped
> section into the generated config, alongside the plain one, every time you
> deploy to something other than the default. You never write it yourself,
> and it never survives a rebuild you didn't ask `bynk deploy` to make.

Under a non-default environment, each Worker, Service Binding target and queue
name takes a `-<env>` suffix. So `staging` and `production` are separate
Workers on the same account, and never share a queue or bind to each other's
Workers. The plan names them as the deploy creates them, in text and in
`--format json` (`order`, each context's `worker` and `binds_to`, and its
queues):

```
queue create job-intake-staging
deploy commerce-orders-staging
order commerce-payment-staging → commerce-orders-staging
```

The plan and the deploy take these names from the same place: the
environment's section of the generated config states the Worker's `name`
(`commerce-orders-staging`) rather than leaving it to Wrangler's default, which
is the same name. Every message the deploy prints names Workers the same way,
and a command it suggests repeats your `--env`: under `--env staging`, the
`--context` refusal reads "`commerce-orders-staging` binds to
`commerce-payment-staging`, which has never been deployed" and suggests
`bynk deploy --env staging`.

In `--format json`, `orphans.workers` and `orphans.queues` hold these
account names too. With the [orphans](#orphans) example's removed `payment`
context and its `old-jobs` queue, under `--env staging` they are
`payment-staging` and `old-jobs-staging`. `orphans.kv` does not:
each entry's `worker` is the Worker's ledger key, its bare name, alongside the
`binding` (absent for the bare `Kv`), because that pair is what the ledger
records the namespace under.

KV's identity is the namespace id Cloudflare hands back, already kept apart per
environment in `bynk.deploy.lock`. A namespace's title takes the suffix too,
because Cloudflare refuses a second namespace with a title the account already
holds. So the bare `Kv`'s namespace is `shop-cart-staging` and a derived
instance's is `shop-cart-sessions-staging`. The default environment's titles
are unchanged (`shop-cart`, `shop-cart-sessions`). The title only matters when
the namespace is created: one whose id the lock file already records for the
environment is reused, whatever it was titled when it was made, and the plan
shows the title Bynk would give it.

Suffixes can still make two titles coincide: Worker `shop`'s bare namespace
under `--env staging` and its `capability Staging` in the default environment
are both `shop-staging`. `bynk deploy` refuses that before the plan,
`--dry-run` included, naming both namespaces and both environments. It
compares the namespaces this deploy would create against every environment
`bynk.deploy.lock` knows, both the namespaces recorded there and the ones that
environment's next deploy would create.

A namespace can also be created again during the deploy itself: one whose
recorded id has gone from the account, deleted outside Bynk, is re-created
under its current title. The plan can't see that, because it runs before
the deploy fetches the account's live ids. So `bynk deploy` makes the same
comparison at that point and stops with the same message before running
`wrangler kv namespace create`.

At that deploy-time check, and only there, another environment's recorded
namespace whose id has gone from the account no longer holds its title,
unless your source still declares it in that environment. The plan-time
check runs before the live ids are fetched, so it counts every namespace
the lock file records. If the plan refuses a title held by an orphan you
already deleted outside Bynk, clear that stale entry. Either run
`bynk deploy --prune` in the orphan's own environment (for example
`bynk deploy --env staging --prune`), which counts a namespace that is
already gone as deleted and drops it from `bynk.deploy.lock` after that
environment's deploy pushes cleanly. Or remove the entry from
`bynk.deploy.lock` by hand.

`--env` is the driver's own flag; putting `--env` or `--environment` after
`--` as well is rejected rather than silently resolved one way or the other:

```sh
bynk deploy --env staging -- --env production
# bynk: `--env staging` conflicts with `--env` after `--` — pass one or the other, not both
```

## Provisioning state

The first deploy writes `bynk.deploy.lock` beside `bynk.toml`. Commit it. It is
intentionally not written into the generated `wrangler.toml`, because each build
replaces that file. It holds no secrets. Every environment gets its own section
of the file, so `staging` and `production` never share a KV id, a deployed-state
flag, or a queue record. Per environment, it records three different things, and
it trusts them to three different degrees:

- **KV namespace ids** — the real state. Cloudflare generates the id, and this
  file is the only place it exists, so every developer and CI job deploys to the
  same namespaces. The bare `Kv`'s are kept by Worker, under `kv`; each derived
  instance's by Worker and binding, under `kv_instances`.
- **Which contexts have been deployed** — how `--context` knows whether the
  contexts you bind to are actually live. With each, the contract hashes it
  provides (`contracts`) and the ones it was compiled against (`expects`), so
  `--context` can refuse a push that would break a live caller or callee.
- **Which queues have been created** — a note to itself, so the plan can say
  `create` or `reuse` without calling Cloudflare. Nothing depends on it being
  right: a queue is found by its name either way.

CI can deploy a project with a recorded namespace. It does not bootstrap a new
one: run the first deploy locally, commit `bynk.deploy.lock`, and then let CI
push subsequent builds. This avoids creating a namespace whose identity cannot
be committed back to the project.

That restriction is about ids, so it applies to KV alone, and to every KV
namespace: an unrecorded derived instance is refused in CI too. CI happily creates a
queue, because a queue's name comes from your source — the next run derives the
same name and finds the same queue, with or without the lock file.

After a first deploy, `bynk dev -- --remote` reads the same lock file to fill
every KV binding, the bare one and each instance's — pass the matching `--env` if you deployed to a named one
(`bynk dev --env staging -- --remote`). Normal local `bynk dev` remains
entirely local and needs no Cloudflare account.

## Orphans

Delete a context, or the last `on queue` handler that used it, and the ledger
still remembers what it provisioned — `bynk deploy` says so before doing
anything else:

```
orphan kv payment
orphan worker payment
orphan queue old-jobs
deploy commerce-orders
```

A removed context with a KV namespace shows up as **two** lines — the
namespace and the Worker are separate resources, tracked separately, and
reported separately. A derived instance is an orphan with its context, or on its
own when its context still deploys but no longer declares it. So is the bare
namespace, once a context stops using the bare `Kv` (`orphan kv api`). Renaming
`capability Carts = Kv` to `Baskets` is a new binding, so the plan creates
`KV_BASKETS` and reports the old one, naming its binding:

```
orphan kv shop-cart KV_CARTS
kv create shop-cart-baskets (KV_BASKETS)
redeploy shop-cart
```

The data is not moved. Copy it across before you prune the old namespace. This is a plain diff against your current source; it
never calls Cloudflare, so it works under `--dry-run` too.

Pass `--prune` to delete what's reported — KV namespaces, queues and the
Hyperdrive configs a deploy made, never a Worker. It does not touch Worker versions or static assets: Cloudflare
owns those, and an asset lives as long as a version that serves it. Anything that
holds data (a D1 database, an R2 bucket) is only ever listed:

```sh
bynk deploy --prune
```

```
bynk: will delete KV namespace for `payment`
bynk: will delete KV namespace `shop-cart-carts` for `shop-cart`'s `KV_CARTS`
bynk: will delete queue `old-jobs`
Delete 3 resource(s)? [y/N]
```

That confirmation is separate from the one `--yes` already skips for
creation — a script that only meant to authorise provisioning must not also,
silently, authorise deletion. Pass `--yes` **and** `--prune` together to
prune non-interactively.

`--prune` is safe to re-run: deleting something already gone (a resource
someone else removed between your plan and your prune) is treated the same
as deleting it now — the ledger entry is stripped either way, so a
half-finished prune never gets stuck re-reporting the same orphan forever.

An orphaned Worker is reported but never deleted — `wrangler delete` also
removes routes, custom domains, and cron triggers, a larger blast radius than
a namespace or a queue. Clean one up by hand with `wrangler delete` if you're
sure it's unused.

Orphans are project-wide, not scoped to `--context`: `bynk deploy --context
orders --prune` can delete a queue orphaned by a *different*, unrelated
context — the report (and the confirmation naming exactly what will be
deleted) is what keeps this visible rather than surprising.

Separately: a KV namespace deleted outside Bynk (in the Cloudflare
dashboard, say) is noticed the next time you deploy and re-provisioned
automatically — the same self-healing a deleted queue already had.

---
title: Deploy it
---
The shortener is tested and runs locally: three files, two contexts, two
Workers. In this last chapter it goes live on Cloudflare. You'll read what a
deploy will do before it does anything, deploy, watch the live service's logs,
and see how to undo a release.

Work in the project from [Test it](/book/tutorials/testing/), with its three
files in `src/`.

## Before you start

Deploying needs two things that the earlier chapters didn't:

- **A Cloudflare account.** The shortener uses Workers and, for its `Link`
  agent, Durable Objects. Check Cloudflare's own pages on
  [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/)
  for what your plan includes; they change, so this chapter doesn't repeat
  them.
- **Wrangler, signed in.** `bynk deploy` drives Cloudflare's own `wrangler`
  tool. Sign in once with `wrangler login`, or set `CLOUDFLARE_API_TOKEN` (and
  `CLOUDFLARE_ACCOUNT_ID`) in an automated setting.

You can do the next step without either.

## Read the plan first

A deploy creates real things on your account, so look before you leap:

```sh
bynk deploy --dry-run
```

`--dry-run` compiles the project and prints what a deploy would do, and changes
nothing, on Cloudflare or in the project. Without credentials it works offline.
For the shortener it prints:

```text
durable object Link (sqlite; advisory — Cloudflare reconciles it)
secret set BYNK_CALL_FROM_SHORTENER (managed)
deploy registry
secret set BYNK_CALL_TO_REGISTRY (managed)
deploy shortener
order registry → shortener
```

Each line is something the code you wrote asked for:

- **`durable object Link`.** On Cloudflare, an agent is a Durable Object: one
  object per key, with its own storage. `Link` from
  [Add a stateful agent](/book/tutorials/stateful-agent/) becomes one, and each
  short code gets its own. The line is *advisory*: Cloudflare creates the
  namespace if your account hasn't got it, so Bynk doesn't claim to know.
- **The two `secret set` lines.** `shortener` calls `registry` across a service
  binding, because of `consumes registry`. Bynk secures that call with a pair
  of secrets it creates and manages, so that only `shortener` can call
  `registry`'s services. You never handle them.
- **`deploy registry`, then `deploy shortener`.** Cloudflare checks a service
  binding when a Worker is uploaded: a Worker that binds to one that doesn't
  exist yet is rejected. So `registry` goes first, and the last line says why.
  Bynk works the order out from your `consumes` declarations; you never write
  it down.

## Deploy

```sh
bynk deploy
```

It checks that Node and Wrangler are installed and that you're signed in,
shows the same plan, and asks before it changes anything. Then it provisions
what the plan lists, deploys both Workers in order, and prints the address of
each. The `shortener` Worker is the one that answers HTTP.

Try it, with the address `bynk deploy` printed in place of `<shortener-url>`:

```sh
curl -X POST <shortener-url>/links \
  -H 'content-type: application/json' \
  -d '{"target":"https://example.com"}'
```

The answer is `201 Created`, with a fresh random code. Follow it with
`curl <shortener-url>/links/<code>`, and the hit count goes up each time. A
target that starts with the shortener's own address gets the `400` from
[Handle failure with `Result`](/book/tutorials/failure-and-result/).

## Watch it

```sh
bynk tail --context shortener
```

`bynk tail` streams a deployed Worker's logs. With two contexts, `--context`
says which one. Each `POST` you send shows the
`Logger.info("created …")` line from
[Reach out with capabilities](/book/tutorials/capabilities/). If anything
throws, the stack frames point at lines of your `.bynk` files, not at the
generated TypeScript.

## Change it, and undo a change

Change something small, such as the `Logger` message, and run `bynk deploy`
again. The plan reads the same, and nothing is lost: each Worker gets a new
version, and the `Link` agents keep their stored links and hit counts.

If a release goes wrong, `bynk revert` makes the previous deployment live again,
without a rebuild:

```sh
bynk revert --dry-run
```

`--dry-run` prints every check and changes nothing. A rollback is refused
whenever it would be unsafe, for example if agent state was already written by
the newer release in a form the older one can't read. Unlike
`bynk deploy --dry-run`, it reads your account to make those checks, so it needs
`CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`. Drop `--dry-run` to roll
back for real. [Roll back a deployment](/book/guides/projects-build-and-deployment/roll-back-a-deployment/)
has the details.

## What you have done

You took the shortener live. `bynk deploy --dry-run` showed what your code asks
of Cloudflare: a Durable Object for the agent, a secured binding between the two
contexts, and the order they must go in. `bynk deploy` made it so,
`bynk tail` showed the live logs, and `bynk revert` is there if a release goes
wrong.

That's the end of Learn Bynk. In ten chapters the shortener went from one
function to a tested, deployed service of two contexts, with typed values,
errors as values, declared effects, state in an agent, and an architecture the
compiler checks.

## Where next

- **[Concepts](/book/concepts/)**, for why Bynk works the way it does.
- **[Guides](/book/guides/)**, for the next thing you're building: storing
  data, calling out, background work, reliability and more.
- **[Reference](/reference/)**, for exact answers.
- **[By Example](/by-example/)**, for short programs to run and complete
  projects to read.
- **[The blog](https://bynk-lang.org/blog/)**, for the thinking behind the language, one idea at a
  time.

---

*Everything `bynk deploy` can provision, and how it handles secrets,
environments and a deploy that fails part-way, is in
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/).
How a context maps to a Worker is in
[Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/),
and every flag is in the [CLI reference](/reference/tooling/bynk-cli/).*

// `bynk-cloudflare`: the `bynk` surface binding's behavioural tests (standard-library S2,
// #1952). The shared suite is `surface.mjs`; this file says which platform
// differences apply.

import * as binding from "./bynk-cloudflare.js";
import * as runtime from "./__bynk_runtime/result.js";
import * as bynk from "./bynk.js";
import { surfaceSuite } from "./surface.mjs";

surfaceSuite("bynk-cloudflare", binding, runtime, bynk, {
  fetch: "real",
  secrets: "env",
  locale: "negotiated",
  log: "object",
});

// `IdempotencyProvider.claim` against a D1 database (#2840). The database is
// SQLite behind the part of D1's API the provider uses, as the compiler's D1 test
// double builds it, with the compiler's own migration applied.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";

function claimsDatabase(sqlSource) {
  const sqlite = createRequire(import.meta.url)("node:sqlite");
  const db = new sqlite.DatabaseSync(":memory:");
  db.exec(sqlSource);
  const statement = (sql, bound) => ({
    sql,
    bind: (...values) => statement(sql, values),
    run: () => db.prepare(sql).run(...bound),
  });
  let failWhen = null;
  return {
    prepare: (sql) => statement(sql, []),
    // Make every batch that has a statement matching `pattern` throw, as a D1 outage would.
    failOn: (pattern) => {
      failWhen = pattern;
    },
    async batch(statements) {
      if (failWhen && statements.some((s) => failWhen.test(s.sql))) throw new Error("D1 is unavailable");
      db.exec("BEGIN");
      try {
        const out = statements.map((s) => ({ meta: { changes: Number(s.run().changes) } }));
        db.exec("COMMIT");
        return out;
      } catch (e) {
        db.exec("ROLLBACK");
        throw e;
      }
    },
    rows: () => db.prepare("SELECT key, expires_at FROM idempotency_claims ORDER BY key").all(),
  };
}

const MIGRATION = readFileSync(new URL("./0001_idempotency_claims.sql", import.meta.url), "utf8");

function clock(start) {
  return { t: start, async now() { return this.t; } };
}

const d1 = (name, fn) => test(`bynk-cloudflare IdempotencyProvider.${name}`, fn);

d1("claim: with a database, the first claim wins and a repeat is AlreadyClaimed until it lapses", async () => {
  const db = claimsDatabase(MIGRATION);
  const c = clock(1_000);
  const idem = new binding.IdempotencyProvider({ Clock: c }, { D1_IDEMPOTENCY: db });
  assert.equal((await idem.claim("k", 500)).tag, "Claimed");
  c.t = 1_499;
  assert.equal((await idem.claim("k", 500)).tag, "AlreadyClaimed");
  c.t = 1_500;
  assert.equal((await idem.claim("k", 500)).tag, "Claimed", "an expired claim is taken over");
});

d1("claim: with a database, claims are shared between instances", async () => {
  const db = claimsDatabase(MIGRATION);
  const env = { D1_IDEMPOTENCY: db };
  const a = new binding.IdempotencyProvider({ Clock: clock(0) }, env);
  const b = new binding.IdempotencyProvider({ Clock: clock(0) }, env);
  assert.equal((await a.claim("k", 1_000)).tag, "Claimed");
  assert.equal((await b.claim("k", 1_000)).tag, "AlreadyClaimed");
});

d1("claim: with a database, concurrent claims of one key yield exactly one Claimed", async () => {
  const db = claimsDatabase(MIGRATION);
  const env = { D1_IDEMPOTENCY: db };
  const all = await Promise.all(
    Array.from({ length: 20 }, () => new binding.IdempotencyProvider({ Clock: clock(0) }, env).claim("k", 1_000)),
  );
  assert.equal(all.filter((c) => c.tag === "Claimed").length, 1);
});

d1("claim: with a database, an expired claim of another key is swept", async () => {
  const db = claimsDatabase(MIGRATION);
  const c = clock(0);
  const idem = new binding.IdempotencyProvider({ Clock: c }, { D1_IDEMPOTENCY: db });
  await idem.claim("old", 10);
  c.t = 100;
  await idem.claim("new", 10);
  assert.deepEqual(db.rows().map((r) => r.key), ["new"]);
});

d1("claim: an env with no database binding fails loudly", async () => {
  const idem = new binding.IdempotencyProvider({ Clock: clock(0) }, {});
  await assert.rejects(idem.claim("k", 1_000), /D1_IDEMPOTENCY/);
});

d1("__track: with a database, a failed handler releases its claim and a completed one confirms it", async () => {
  const db = claimsDatabase(MIGRATION);
  const c = clock(0);
  const idem = new binding.IdempotencyProvider({ Clock: c }, { D1_IDEMPOTENCY: db });
  const failing = idem.__track();
  failing.note("a", await idem.claim("a", failing.lease(1_000_000)), 1_000_000);
  await failing.settle({ tag: "Err" }, false);
  assert.equal((await idem.claim("a", 30_000)).tag, "Claimed", "released");
  const done = idem.__track();
  done.note("b", await idem.claim("b", done.lease(1_000_000)), 1_000_000);
  await done.settle({ tag: "Ok" }, false);
  c.t = 999_999;
  assert.equal((await idem.claim("b", 1)).tag, "AlreadyClaimed", "kept for the full window");
});

d1("__track: with a database, a slow handler leaves the retry's claim alone", async () => {
  const db = claimsDatabase(MIGRATION);
  const c = clock(0);
  const env = { D1_IDEMPOTENCY: db };
  const first = new binding.IdempotencyProvider({ Clock: c }, env);
  const second = new binding.IdempotencyProvider({ Clock: c }, env);
  const slow = first.__track();
  slow.note("k", await first.claim("k", 30_000), 1_000_000);
  c.t = 30_000;
  second.__track().note("k", await second.claim("k", 30_000), 1_000_000);
  await slow.settle({ tag: "Err" }, false);
  assert.equal((await second.claim("k", 30_000)).tag, "AlreadyClaimed");
});

d1("__track: a database error while settling does not fail the handler or skip the other claims", async () => {
  const db = claimsDatabase(MIGRATION);
  const c = clock(0);
  const idem = new binding.IdempotencyProvider({ Clock: c }, { D1_IDEMPOTENCY: db });
  const scope = idem.__track();
  scope.note("a", await idem.claim("a", scope.lease(1_000_000)), 1_000_000);
  scope.note("b", await idem.claim("b", scope.lease(1_000_000)), 1_000_000);
  const logged = [];
  const original = console.error;
  console.error = (...args) => logged.push(args.join(" "));
  try {
    db.failOn(/^UPDATE|^DELETE/);
    await scope.settle({ tag: "Ok" }, false); // resolves: the handler's outcome is kept
    await scope.settle(undefined, true);
  } finally {
    console.error = original;
  }
  assert.ok(logged.length >= 2, "each failed key is logged");
  db.failOn(null);
  c.t = 30_000;
  assert.equal((await idem.claim("a", 1)).tag, "Claimed", "an unsettled claim lapses with its lease");
});

d1("__track: confirming a claim whose lapsed row was swept restores it", async () => {
  const db = claimsDatabase(MIGRATION);
  const c = clock(0);
  const env = { D1_IDEMPOTENCY: db };
  const slow = new binding.IdempotencyProvider({ Clock: c }, env);
  const other = new binding.IdempotencyProvider({ Clock: c }, env);
  const scope = slow.__track();
  scope.note("k", await slow.claim("k", scope.lease(1_000_000)), 1_000_000);
  c.t = 40_000; // the lease lapsed
  await other.claim("elsewhere", 1_000); // another claim sweeps the expired row
  assert.deepEqual(db.rows().map((r) => r.key), ["elsewhere"], "the lapsed row was swept");
  await scope.settle({ tag: "Ok" }, false);
  assert.equal((await other.claim("k", 1)).tag, "AlreadyClaimed", "the slow handler's confirm holds");
});

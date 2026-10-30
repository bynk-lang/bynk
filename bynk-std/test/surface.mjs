// The behavioural suite every `bynk` surface binding runs (standard-library S2,
// #1952). Each `<binding>.test.mjs` calls `surfaceSuite` with its compiled
// binding module and the platform's documented differences: whether `Fetch` is
// real or withheld, whether `Secrets` reads the environment or is unavailable,
// and whether `Locale` negotiates or is fixed.
//
// Fakes follow the platform contract, never the binding under test: `fetch` is
// stubbed with the Fetch standard's own `Response` and `Headers`, a clock is a
// plain `{ now }`, and the environment is a plain object.
//
// Every test is named `<binding> <Provider>.<method>: <claim>`. The coverage
// guard in `bynkc/tests/binding_behaviour.rs` reads those names from the run and
// fails if a provider method has none.

import { test } from "node:test";
import assert from "node:assert/strict";

/** The `Uuid` refinement, as `bynk.bynk` declares it. */
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** A clock that reads `now` until a test moves it. */
function fakeClock(start) {
  const clock = { t: start, async now() { return this.t; } };
  return clock;
}

/** Replace `console[method]` for the duration of `body`, returning what it saw. */
async function capture(method, body) {
  const original = console[method];
  const seen = [];
  console[method] = (...args) => seen.push(args.join(" "));
  try {
    await body();
  } finally {
    console[method] = original;
  }
  return seen;
}

/** As `capture`, keeping each call's arguments as they were passed. */
async function captureArgs(method, body) {
  const original = console[method];
  const seen = [];
  console[method] = (...args) => seen.push(args);
  try {
    await body();
  } finally {
    console[method] = original;
  }
  return seen;
}

/** Replace `globalThis.fetch` for the duration of `body`, returning its calls. */
async function withFetch(stub, body) {
  const original = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push({ url, init });
    return stub(url, init);
  };
  try {
    await body(calls);
  } finally {
    globalThis.fetch = original;
  }
}

/** Set `process.env[name]` for the duration of `body`. */
async function withProcessEnv(name, value, body) {
  const had = Object.prototype.hasOwnProperty.call(process.env, name);
  const previous = process.env[name];
  if (value === undefined) {
    delete process.env[name];
  } else {
    process.env[name] = value;
  }
  try {
    await body();
  } finally {
    if (had) {
      process.env[name] = previous;
    } else {
      delete process.env[name];
    }
  }
}

/**
 * Register the surface suite for one binding.
 *
 * @param {string} name   the binding's file stem, e.g. `bynk-node`
 * @param {object} b      the compiled binding module
 * @param {object} rt     the compiled `runtime.js` (`Some`, `None`)
 * @param {object} bynk   the compiled `bynk.js` (`Method`)
 * @param {object} shape  `{ fetch: "real" | "withheld",
 *                           secrets: "env" | "unavailable",
 *                           locale: "fixed" | "negotiated" }`
 */
export function surfaceSuite(name, b, rt, bynk, shape) {
  const { Some, None } = rt;
  const t = (claim, fn) => test(`${name} ${claim}`, fn);

  // --- Clock -------------------------------------------------------------

  t("ClockProvider.now: returns the current epoch milliseconds", async () => {
    const before = Date.now();
    const now = await new b.ClockProvider().now();
    const after = Date.now();
    assert.equal(typeof now, "number");
    assert.ok(Number.isInteger(now));
    assert.ok(before <= now && now <= after, `${before} <= ${now} <= ${after}`);
  });

  // --- Random ------------------------------------------------------------

  t("RandomProvider.uuid: yields a value satisfying the Uuid refinement, fresh each call", async () => {
    const random = new b.RandomProvider();
    const seen = new Set();
    for (let i = 0; i < 50; i++) {
      const id = await random.uuid();
      assert.match(id, UUID);
      seen.add(id);
    }
    assert.equal(seen.size, 50);
  });

  t("RandomProvider.int: draws integers in [lo, hi), hi exclusive", async () => {
    const random = new b.RandomProvider();
    const seen = new Set();
    for (let i = 0; i < 400; i++) {
      const n = await random.int(3, 7);
      assert.ok(Number.isInteger(n), `${n} is an integer`);
      assert.ok(n >= 3 && n < 7, `${n} in [3, 7)`);
      seen.add(n);
    }
    assert.deepEqual([...seen].sort(), [3, 4, 5, 6]);
    assert.equal(await random.int(-2, -1), -2);
  });

  t("RandomProvider.bytes: returns n random bytes and refuses n outside 1..=65536, naming the bound", async () => {
    const random = new b.RandomProvider();
    const a = await random.bytes(32);
    const c = await random.bytes(32);
    assert.ok(a instanceof Uint8Array);
    assert.equal(a.length, 32);
    assert.notDeepEqual([...a], [...c]);
    assert.equal((await random.bytes(1)).length, 1);
    assert.equal((await random.bytes(65536)).length, 65536);
    for (const bad of [0, -1, 65537, 1.5]) {
      await assert.rejects(() => random.bytes(bad), /1 <= n <= 65536/);
    }
  });

  // --- Logger ------------------------------------------------------------

  t("LoggerProvider.info: writes the message to standard output", async () => {
    const logger = new b.LoggerProvider();
    const out = await capture("log", () => logger.info("hello info"));
    assert.deepEqual(out, ["hello info"]);
  });

  t("LoggerProvider.error: writes the message to standard error", async () => {
    const logger = new b.LoggerProvider();
    const err = await capture("error", () => logger.error("hello error"));
    assert.deepEqual(err, ["hello error"]);
  });

  // K5a (#2586).
  t("LoggerProvider.warn: writes the message with console.warn", async () => {
    const logger = new b.LoggerProvider();
    assert.deepEqual(await capture("warn", () => logger.warn("hello warn")), ["hello warn"]);
  });

  t("LoggerProvider.debug: writes the message with console.debug", async () => {
    const logger = new b.LoggerProvider();
    assert.deepEqual(await capture("debug", () => logger.debug("hello debug")), ["hello debug"]);
  });

  t("LoggerProvider.log: writes a structured record through the level's console method", async () => {
    const logger = new b.LoggerProvider();
    const fields = new Map([["orderId", "o-1"], ["message", "a field, not the message"]]);
    const seen = await captureArgs("warn", () => logger.log(bynk.LogLevel.Warn, "slow order", fields));
    const record = { message: "slow order", level: "warn", fields: { orderId: "o-1", message: "a field, not the message" } };
    if (shape.log === "object") {
      assert.deepEqual(seen, [[record]]);
    } else if (shape.log === "json") {
      assert.deepEqual(seen.map((args) => JSON.parse(args[0])), [record]);
    } else {
      assert.deepEqual(seen, [["slow order", record.fields]]);
    }
    // Each level has its own method.
    for (const [level, method] of [["Debug", "debug"], ["Info", "log"], ["Error", "error"]]) {
      const lines = await captureArgs(method, () => logger.log(bynk.LogLevel[level], "m", new Map()));
      assert.equal(lines.length, 1, level);
    }
  });

  // --- Fetch -------------------------------------------------------------

  const request = (overrides = {}) => ({
    method: bynk.Method.Get,
    url: "https://upstream.test/thing",
    contentType: None,
    authorization: None,
    headers: new Map(),
    body: None,
    ...overrides,
  });

  if (shape.fetch === "real") {
    t("FetchProvider.send: sends the method, url and body, and returns status and body", async () => {
      await withFetch(
        () => new Response("created", { status: 201 }),
        async (calls) => {
          const res = await new b.FetchProvider().send(
            request({ method: bynk.Method.Post, body: Some("payload") }),
          );
          assert.equal(res.tag, "Ok");
          assert.equal(res.value.status, 201);
          assert.equal(res.value.body, "created");
          assert.equal(calls.length, 1);
          assert.equal(calls[0].url, "https://upstream.test/thing");
          assert.equal(calls[0].init.method, "POST");
          assert.equal(calls[0].init.body, "payload");
        },
      );
    });

    t("FetchProvider.send: a failed request is Err(Network)", async () => {
      await withFetch(
        () => { throw new TypeError("fetch failed"); },
        async () => {
          const res = await new b.FetchProvider().send(request());
          assert.equal(res.tag, "Err");
          assert.equal(res.error.tag, "Network");
        },
      );
    });

    t("FetchProvider.send: a timed-out or aborted request is Err(Timeout)", async () => {
      for (const errorName of ["TimeoutError", "AbortError"]) {
        await withFetch(
          () => { throw new DOMException("gave up", errorName); },
          async () => {
            const res = await new b.FetchProvider().send(request());
            assert.equal(res.tag, "Err");
            assert.equal(res.error.tag, "Timeout", errorName);
          },
        );
      }
    });

    // K5a (#2586).
    t("FetchProvider.send: sends PATCH, HEAD and OPTIONS upper-cased, following redirects with no timeout", async () => {
      await withFetch(
        () => new Response(null, { status: 204 }),
        async (calls) => {
          for (const m of ["Patch", "Head", "Options"]) {
            const res = await new b.FetchProvider().send(request({ method: bynk.Method[m] }));
            assert.equal(res.tag, "Ok");
            assert.equal(res.value.body, "");
          }
          assert.deepEqual(calls.map((c) => c.init.method), ["PATCH", "HEAD", "OPTIONS"]);
          assert.ok(calls.every((c) => c.init.redirect === "follow" && c.init.signal === undefined));
        },
      );
    });

    t("FetchProvider.sendWith: a manual redirect returns the 3xx with its location", async () => {
      await withFetch(
        () => new Response(null, { status: 302, headers: { location: "https://elsewhere.test/" } }),
        async (calls) => {
          const res = await new b.FetchProvider().sendWith(request(), {
            timeout: None,
            redirect: bynk.Redirect.Manual,
          });
          assert.equal(res.tag, "Ok");
          assert.equal(res.value.status, 302);
          assert.equal(res.value.headers.get("location"), "https://elsewhere.test/");
          assert.equal(calls[0].init.redirect, "manual");
        },
      );
    });

    t("FetchProvider.sendWith: a timeout that elapses is Err(Timeout)", async () => {
      // A fetch that answers only when its signal aborts, as the platform does.
      await withFetch(
        (_url, init) =>
          new Promise((_resolve, reject) => {
            init.signal.addEventListener("abort", () => reject(init.signal.reason));
          }),
        async (calls) => {
          const res = await new b.FetchProvider().sendWith(request(), {
            timeout: Some(20),
            redirect: bynk.Redirect.Follow,
          });
          assert.equal(res.tag, "Err");
          assert.equal(res.error.tag, "Timeout");
          assert.equal(calls[0].init.redirect, "follow");
        },
      );
    });

    t("FetchProvider.sendWith: a timeout of zero or less faults, and nothing is sent", async () => {
      await withFetch(
        () => new Response("unexpected"),
        async (calls) => {
          for (const ms of [0, -5]) {
            await assert.rejects(
              new b.FetchProvider().sendWith(request(), { timeout: Some(ms), redirect: bynk.Redirect.Follow }),
              /a timeout is more than zero/,
            );
          }
          assert.equal(calls.length, 0);
        },
      );
    });
  } else {
    t("FetchProvider.sendWith: is withheld — it throws, and nothing is sent", async () => {
      await withFetch(
        () => new Response("unexpected"),
        async (calls) => {
          await assert.rejects(
            new b.FetchProvider().sendWith(request(), { timeout: None, redirect: bynk.Redirect.Follow }),
            /Fetch is unavailable/,
          );
          assert.equal(calls.length, 0);
        },
      );
    });

    t("FetchProvider.send: is withheld — it throws, and nothing is sent", async () => {
      await withFetch(
        () => new Response("unexpected"),
        async (calls) => {
          await assert.rejects(new b.FetchProvider().send(request()), /Fetch is unavailable/);
          assert.equal(calls.length, 0);
        },
      );
    });
  }

  // --- Secrets -----------------------------------------------------------

  if (shape.secrets === "env") {
    t("SecretsProvider.get: reads the explicit env before process.env", async () => {
      await withProcessEnv("BYNK_S2_SECRET", "from-process", async () => {
        const got = await new b.SecretsProvider({ BYNK_S2_SECRET: "from-env" }).get("BYNK_S2_SECRET");
        assert.deepEqual(got, Some("from-env"));
      });
    });

    t("SecretsProvider.get: falls back to process.env when the env lacks the name", async () => {
      await withProcessEnv("BYNK_S2_SECRET", "from-process", async () => {
        assert.deepEqual(await new b.SecretsProvider({}).get("BYNK_S2_SECRET"), Some("from-process"));
        assert.deepEqual(await new b.SecretsProvider().get("BYNK_S2_SECRET"), Some("from-process"));
        // A non-string env value is not a secret; the lookup falls through.
        assert.deepEqual(
          await new b.SecretsProvider({ BYNK_S2_SECRET: 42 }).get("BYNK_S2_SECRET"),
          Some("from-process"),
        );
      });
    });

    t("SecretsProvider.get: an unset name is None", async () => {
      await withProcessEnv("BYNK_S2_SECRET", undefined, async () => {
        assert.deepEqual(await new b.SecretsProvider({}).get("BYNK_S2_SECRET"), None);
      });
    });
  } else {
    t("SecretsProvider.get: is unavailable — it throws rather than answering None", async () => {
      await withProcessEnv("BYNK_S2_SECRET", "from-process", async () => {
        await assert.rejects(
          new b.SecretsProvider({ BYNK_S2_SECRET: "from-env" }).get("BYNK_S2_SECRET"),
          /Secrets is unavailable/,
        );
      });
    });
  }

  // --- Locale ------------------------------------------------------------

  t("LocaleProvider.current: with no request to negotiate from, is the reference tag en", async () => {
    assert.equal(await new b.LocaleProvider().current(), "en");
  });

  if (shape.locale === "negotiated") {
    t("LocaleProvider.current: negotiates Accept-Language against the declared locales", async () => {
      const ask = (header) =>
        new b.LocaleProvider(
          new Request("https://app.test/", { headers: header ? { "accept-language": header } : {} }),
          ["en", "fr", "pt"],
          "en",
        ).current();
      assert.equal(await ask("fr"), "fr");
      assert.equal(await ask("pt-BR"), "pt", "rightmost-subtag truncation");
      assert.equal(await ask("de, fr;q=0.5"), "fr");
      assert.equal(await ask("de"), "en", "no match falls back to the reference");
      assert.equal(await ask(undefined), "en", "no header falls back to the reference");
    });
  }

  // --- Idempotency -------------------------------------------------------

  t("IdempotencyProvider.dedup: is None for a key never remembered", async () => {
    const idem = new b.IdempotencyProvider({ Clock: fakeClock(1_000) });
    assert.deepEqual(await idem.dedup("never"), None);
  });

  t("IdempotencyProvider.dedup: answers a remembered value until it expires", async () => {
    const clock = fakeClock(1_000);
    const idem = new b.IdempotencyProvider({ Clock: clock });
    await idem.remember("k", { order: 7 }, 500);
    clock.t = 1_499;
    assert.deepEqual(await idem.dedup("k"), Some({ order: 7 }));
    clock.t = 1_500;
    assert.deepEqual(await idem.dedup("k"), None, "expired exactly at now + expiresAfter");
  });

  t("IdempotencyProvider.remember: a later remember replaces the value and restarts the window", async () => {
    const clock = fakeClock(0);
    const idem = new b.IdempotencyProvider({ Clock: clock });
    await idem.remember("k", "first", 100);
    clock.t = 90;
    await idem.remember("k", "second", 100);
    clock.t = 150;
    assert.deepEqual(await idem.dedup("k"), Some("second"));
  });
}

//! 1B.2b (#4028): the platform-capability emission tests that drove the driver's in-memory
//! pipeline, moved from `bynk-emit` (`emitter/lower_call/platform.rs`).

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;
use bynk_check::project_model::BuildTarget;
use emit_pipeline::{emit_project, emit_workers};

const LEDGER: &str = "context ledger

consumes bynk { Events }

event Bumped = {
  amount: Int,
}

agent Ledger {
  key id: String
  store total: Cell[Int]

  on call bump(amount: Int) -> Effect[Int] given Events {
    let next = total + amount
    total := next
    let _ <- Events.emit[Bumped](Bumped { amount: amount })
    next
  }
}
";

const HOOKS: &str = "context shop.hooks

consumes bynk { Idempotency, Claim }

service api from http {
  on POST(\"/hooks/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Idempotency {
    let Claim.Claimed <- Idempotency.claim(id, 1.hour) else { Ok(\"duplicate\") }
    Ok(\"processed\")
  }
}

service replay from http {
  on POST(\"/replay/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Idempotency {
    let cached <- Idempotency.dedup[String](id)
    match cached {
      Some(v) => Ok(v),
      None => {
        let _ <- Idempotency.remember[String](id, \"computed\", 1.hour)
        Ok(\"fresh\")
      }
    }
  }
}

fn once[T: Codable](key: String, value: T) -> Effect[Option[T]] given Idempotency {
  let seen <- Idempotency.dedup[T](key)
  match seen {
    Some(v) => Some(v),
    None => None,
  }
}

type Receipt = { id: String, total: Int }

service skew from http {
  on POST(\"/receipt/:id\") (id: String) -> Effect[HttpResult[String]] by Visitor given Idempotency {
    let r <- once[Receipt](id, Receipt { id: id, total: 42 })
    match r {
      Some(v) => Ok(\"hit\"),
      None => Ok(\"stored\"),
    }
  }
}
";

const COOKIES: &str = "context site.prefs

consumes bynk { Cookie }

type Theme = enum { Light, Dark }
type Qty = Int where Positive

type Prefs = {
	theme: Theme = Light,
	@name(\"__Host-visits\") visits: Int = 0,
	price: Option[Decimal],
	qty: Option[Qty],
}

service site from http {
	on GET(\"/visit\") (cookies: Prefs) -> Effect[HttpResult[String]] by v: Visitor {
		Ok(\"visit\")
	}

	on POST(\"/leaves\") () -> Effect[HttpResult[()]] by v: Visitor {
		NoContent
			.withCookie(Prefs.price.cookie(1.50d))
			.withCookie(Prefs.visits.cookie(42))
			.withCookie(Prefs.qty.cookie(7))
	}

	on POST(\"/theme/reset\") () -> Effect[HttpResult[()]] by v: Visitor {
		NoContent.withCookie(Prefs.theme.expire())
	}
}
";

const SESSION: &str = "context shop.api

consumes bynk { Cookie, Clock }

type UserId = String where NonEmpty && MaxLength(64)

type Login = { user: String }

actor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }

service api from http {
	on POST(\"/login\") (body: Login) -> Effect[HttpResult[String]] by v: Visitor given Clock {
		let Ok(id) = UserId.of(body.user) else {
			BadRequest(\"bad user\")
		}
		let c <- User.issue(id)
		Created(\"welcome\").withCookie(c)
	}

	on POST(\"/logout\") () -> Effect[HttpResult[()]] by u: User {
		NoContent.withCookie(User.expire())
	}
}
";

const STOCK: &str = "context shop.stock

consumes bynk
consumes bynk.cloudflare { D1, SqlValue, SqlReadError, SqlError }

capability Stock = D1

type Item = { id: String, qty: Int, note: Option[String] }

service listing {
	on call(min: Int) -> Effect[Result[List[Item], SqlReadError]] by Caller given Stock {
		Stock.select[Item](\"FROM items WHERE note = 'caf\\u{e9} \\\"q\\\"' AND qty >= ?1 ORDER BY id\", [Integer(min)])
	}
}

service lookup {
	on call(id: String) -> Effect[Result[Option[Item], SqlReadError]] by Caller given Stock {
		Stock.firstAs[Item](\"FROM items WHERE id = ?1\", [Text(id)])
	}
}
";

fn handlers(file: &str, source: &str, path: &str) -> String {
    emit_workers(&[(file, source)]).text(path).to_string()
}

/// `Events.emit`: the envelope is one block-bodied IIFE, with the event name and the
/// publisher as plain strings. On `workers` the payload goes out in its wire form (#4632).
#[test]
fn events_emit_is_a_block_iife_with_the_envelope() {
    let ts = handlers("ledger.bynk", LEDGER, "workers/ledger/handlers.ts");
    assert!(
            ts.contains(
                "await (async () => {\n        __events.push({ type: \"Bumped\", payload: __serialise_Bumped({ amount: amount }), envelope: { eventId: globalThis.crypto.randomUUID(), publisherId: \"ledger\", emittedAt: globalThis.Date.now(), schemaVersion: 1, trace: __currentTraceId() } });\n      })();"
            ),
            "{ts}"
        );
}

/// First-party `Idempotency` scopes its key once, as a template literal; a tracked `claim`
/// brackets its `await` (the shape #3725's table gets wrong) and wraps the handler in the
/// claim scope; a generic call encodes with the caller's witness.
#[test]
fn idempotency_keys_are_scoped_once_and_claims_are_tracked() {
    let ts = handlers("shop/hooks.bynk", HOOKS, "workers/shop-hooks/handlers.ts");
    assert!(
            ts.contains("(await (async (__k: string, __t: number) => __claims.note(__k, await deps.Idempotency.claim(__k, __claims.lease(__t)), __t))(`shop.hooks.api.http_POST_hooks_Param_id::${id}`, 3600000))"),
            "{ts}"
        );
    assert!(ts.contains("const __claims = "), "{ts}");
    // `wire`: the type argument is dropped, the reply decodes under the `miss` policy, and an
    // encoded argument is the serialised value.
    assert!(
            ts.contains("__bynkCapabilityReply(deps.Idempotency.dedup(`shop.hooks.replay.http_POST_replay_Param_id::${id}`), { encode: __serialise_Option_String, decode: __deserialise_Option_String }, { capability: \"Idempotency\", operation: \"dedup\", type: \"Option[String]\" }, \"miss\")"),
            "{ts}"
        );
    assert!(
            ts.contains("deps.Idempotency.remember(`shop.hooks.replay.http_POST_replay_Param_id::${id}`, \"computed\" as __JsonValue, 3600000)"),
            "{ts}"
        );
    // A generic body encodes with the caller's witness and is scoped to the function.
    assert!(
        ts.contains(
            "deps.Idempotency.dedup(`shop.hooks.fn.once::${key}`), __bynkCodecOption(__codec_T)"
        ),
        "{ts}"
    );
}

/// A typed cookie over each leaf kind; a `Decimal` wraps and imports its helper; expiry is
/// the deletion literal.
#[test]
fn typed_cookies_cover_each_leaf_kind() {
    let ts = handlers("site/prefs.bynk", COOKIES, "workers/site-prefs/handlers.ts");
    for want in [
        "value: __cookieValue(\"decimal\", __bynkDecimalToString({ coefficient: 150n, scale: 2 }))",
        "value: __cookieValue(\"int\", 42)",
        "value: __cookieValue(\"int\", (7 as Qty))",
        "name: \"__Host-visits\"",
        "({ name: \"theme\", value: \"\", maxAge: Some(0), expires: Some(0), domain: { tag: \"None\" as const }, path: \"/\", secure: true, httpOnly: true, sameSite: { tag: \"Lax\" as const }, partitioned: false })",
    ] {
        assert!(ts.contains(want), "missing {want}\n{ts}");
    }
    assert!(ts.contains("__bynkDecimalToString"), "{ts}");
    assert!(
        ts.lines()
            .any(|l| l.starts_with("import") && l.contains("__bynkDecimalToString")),
        "{ts}"
    );
}

/// A signed-cookie issue is an async-arrow IIFE over the runtime helper, and expiry a plain
/// call.
#[test]
fn signed_cookies_issue_and_expire() {
    let ts = handlers("shop/api.bynk", SESSION, "workers/shop-api/handlers.ts");
    assert!(
            ts.contains("(async () => __issueSessionCookie(__sessionKeysOf(deps, \"User\"), \"User\", \"__Host-session\", \"Lax\", id, 43200, await deps.Clock.now()))()"),
            "{ts}"
        );
    assert!(
        ts.contains("__expireSessionCookie(\"__Host-session\", \"Lax\")"),
        "{ts}"
    );
}

/// `D1.select` and `firstAs`: a block-bodied IIFE, the SQL text intact (a non-ASCII character
/// and escaped quotes included), the error tags `as const`.
#[test]
fn d1_select_and_first_as_build_block_iifes() {
    let ts = handlers("shop/stock.bynk", STOCK, "workers/shop-stock/handlers.ts");
    assert!(
            ts.contains("const __r = await deps.Stock.all(\"SELECT \\\"id\\\", \\\"qty\\\", \\\"note\\\" FROM items WHERE note = 'caf\u{e9} \\\"q\\\"' AND qty >= ?1 ORDER BY id\", [SqlValue.Integer(min)]);"),
            "{ts}"
        );
    assert!(
            ts.contains("      if (__r.tag === \"Err\") return Err({ tag: \"Sql\" as const, error: __r.error });\n      const __d = __decode(__r.value);"),
            "{ts}"
        );
    assert!(ts.contains("await deps.Stock.first(\"SELECT"), "{ts}");
    assert!(
        ts.contains("if (__r.value.tag === \"None\") return Ok(None);"),
        "{ts}"
    );
    assert!(
        ts.contains("Err({ tag: \"Decode\" as const, error: __d.error })"),
        "{ts}"
    );
}

/// The emitted build also compiles for the bundle target (`D1.peek` aside).
#[test]
fn the_bundle_target_emits_events_too() {
    let out = emit_project(&[("ledger.bynk", LEDGER)], BuildTarget::Bundle);
    assert!(
        out.docs
            .values()
            .any(|t| t.contains("__events.push({ type: \"Bumped\""))
    );
}

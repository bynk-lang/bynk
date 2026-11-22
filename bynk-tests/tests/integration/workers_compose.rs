//! 1B.2b (#4028): the `compose.ts` / `handlers.ts` tests that drove the driver's in-memory
//! Workers pipeline, moved from `bynk-emit` (`emitter/workers.rs`) so that crate has no
//! dev-dependency on `bynk-driver`.

#[path = "../support/emit_pipeline.rs"]
mod emit_pipeline;
use emit_pipeline::emit_workers;

const PAYMENT: &str = "context demo.payment

exports transparent { Receipt }

type Receipt = { id: String }

service authorise {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    Ok(Receipt { id: id })
  }
}
";

const CHECKOUT: &str = "context demo.checkout

consumes demo.payment

service pay {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    demo.payment.authorise(id)
  }
}
";

fn compose(files: &[(&str, &str)], worker: &str) -> String {
    emit_workers(files)
        .text(&format!("workers/{worker}/compose.ts"))
        .to_string()
}

#[test]
fn each_call_service_gets_a_wrapper_delegating_to_its_handler() {
    let ts = compose(&[("demo/payment.bynk", PAYMENT)], "demo-payment");
    assert!(ts.contains("export function compose(env: Env) {"), "{ts}");
    assert!(
        ts.contains(
            "async authorise(id: string) {\n      return handlers.authorise.call(id, deps);"
        ),
        "{ts}"
    );
}

#[test]
fn a_context_consuming_nothing_has_an_empty_env_and_no_env_in_deps() {
    let ts = compose(&[("demo/payment.bynk", PAYMENT)], "demo-payment");
    assert!(ts.contains("export interface Env {\n}"), "{ts}");
    assert!(ts.contains("const deps = {  };"), "{ts}");
}

#[test]
fn a_consumed_context_is_a_service_binding_and_env_reaches_the_handlers() {
    let ts = compose(
        &[
            ("demo/payment.bynk", PAYMENT),
            ("demo/checkout.bynk", CHECKOUT),
        ],
        "demo-checkout",
    );
    assert!(ts.contains("  DEMO_PAYMENT: __ServiceBinding;"), "{ts}");
    assert!(ts.contains("const deps = { env };"), "{ts}");
}

/// #1961: a context consumed only for its types is not called, so it gets
/// no Service Binding and its handlers' `env` does not name one.
#[test]
fn a_consumed_context_that_is_never_called_is_not_bound() {
    let ledger = "context demo.ledger

consumes demo.payment

service keep {
  on call(r: Receipt) -> Effect[Int] {
    Effect.pure(1)
  }
}
";
    let out = emit_workers(&[("demo/payment.bynk", PAYMENT), ("demo/ledger.bynk", ledger)]);
    let ts = out.text("workers/demo-ledger/compose.ts");
    assert!(!ts.contains("DEMO_PAYMENT"), "{ts}");
    assert!(ts.contains("export interface Env {\n}"), "{ts}");
    let toml = out.text("workers/demo-ledger/wrangler.toml");
    assert!(!toml.contains("[[services]]"), "{toml}");
}

/// #1961: each handler's `deps.env` names only the context it calls, while
/// the Worker's `Env` binds both, so `compose`'s `env` satisfies each.
#[test]
fn each_handler_env_names_only_the_contexts_it_calls() {
    let ledger = "context demo.ledger

service note {
  on call(id: String) -> Effect[Int] {
    Effect.pure(1)
  }
}
";
    let hub = "context demo.hub

consumes demo.payment
consumes demo.ledger

service pay {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    demo.payment.authorise(id)
  }
}

service log {
  on call(id: String) -> Effect[Int] {
    demo.ledger.note(id)
  }
}
";
    let out = emit_workers(&[
        ("demo/payment.bynk", PAYMENT),
        ("demo/ledger.bynk", ledger),
        ("demo/hub.bynk", hub),
    ]);
    let ts = out.text("workers/demo-hub/compose.ts");
    assert!(ts.contains("  DEMO_LEDGER: __ServiceBinding;"), "{ts}");
    assert!(ts.contains("  DEMO_PAYMENT: __ServiceBinding;"), "{ts}");
    let handlers = out.text("workers/demo-hub/handlers.ts");
    let env_of = |svc: &str| {
        let at = handlers
            .find(&format!("export const {svc} = {{"))
            .unwrap_or_else(|| panic!("no `{svc}`:\n{handlers}"));
        let rest = &handlers[at..];
        let start = rest.find("env: {").expect("an env type");
        let end = rest[start..].find('}').expect("closed");
        rest[start..start + end].to_string()
    };
    assert!(env_of("pay").contains("DEMO_PAYMENT") && !env_of("pay").contains("DEMO_LEDGER"));
    assert!(env_of("log").contains("DEMO_LEDGER") && !env_of("log").contains("DEMO_PAYMENT"));
}

/// #1944: `Env` declares exactly the bindings `wrangler.toml` does, in
/// the same order: services, the KV namespace, then the Durable Objects
/// by class name, the events fan-out (`__EventsFanout`) after an agent.
#[test]
fn env_declares_the_wrangler_bindings_in_order() {
    let hub = "context ops.hub

consumes bynk { Events }
consumes bynk.cloudflare { Kv }
consumes demo.payment

event Pinged = { id: String }

agent Ledger {
  key id: String

  store seen: Cell[Int]

  on call total() -> Effect[Int] {
    Effect.pure(seen)
  }
}

service ping {
  on call(id: String) -> Effect[()] given Events, Kv {
    let _ <- Kv.put(id, id)
    let _ <- demo.payment.authorise(id)
    Events.emit[Pinged](Pinged { id: id })
  }
}
";
    let out = emit_workers(&[("demo/payment.bynk", PAYMENT), ("ops/hub.bynk", hub)]);
    let ts = out.text("workers/ops-hub/compose.ts");
    let env = ts
        .split_once("export interface Env {\n")
        .and_then(|(_, rest)| rest.split_once("}"))
        .map(|(members, _)| members)
        .unwrap_or_else(|| panic!("no Env interface:\n{ts}"));
    assert_eq!(
        env,
        "  DEMO_PAYMENT: __ServiceBinding;
  KV: __KVNamespace;
  LEDGER: __DurableObjectNamespace;
  ___EVENTS_FANOUT: __DurableObjectNamespace;
",
        "{ts}"
    );
    let toml = out.text("workers/ops-hub/wrangler.toml");
    let declared: Vec<&str> = toml
        .lines()
        .filter_map(|l| {
            l.strip_prefix("binding = \"")
                .or_else(|| l.strip_prefix("name = \""))
        })
        .filter_map(|rest| rest.strip_suffix('"'))
        .filter(|name| *name != "ops-hub")
        .collect();
    let members: Vec<&str> = env
        .lines()
        .filter_map(|l| l.trim().split_once(':').map(|(name, _)| name))
        .collect();
    assert_eq!(members, declared, "{toml}");
}

/// #1950: a handler's `deps.env` type names a subset of the Worker's
/// `Env` members, by the same names and types: the consumed context and
/// the agent it constructs, never the KV namespace or the events fan-out.
#[test]
fn a_handlers_deps_env_is_a_projection_of_env() {
    let hub = "context ops.hub

consumes bynk { Events }
consumes bynk.cloudflare { Kv }
consumes demo.payment

event Pinged = { id: String }

agent Ledger {
  key id: String

  store seen: Cell[Int]

  on call total() -> Effect[Int] {
    Effect.pure(seen)
  }
}

service ping {
  on call(id: String) -> Effect[Int] given Events, Kv {
    let _ <- Kv.put(id, id)
    let _ <- demo.payment.authorise(id)
    let n <- Ledger(id).total()
    let _ <- Events.emit[Pinged](Pinged { id: id })
    Effect.pure(n)
  }
}
";
    let out = emit_workers(&[("demo/payment.bynk", PAYMENT), ("ops/hub.bynk", hub)]);
    let compose = out.text("workers/ops-hub/compose.ts");
    let env: Vec<&str> = compose
        .split_once("export interface Env {\n")
        .and_then(|(_, rest)| rest.split_once("}"))
        .map(|(members, _)| members.lines().map(|l| l.trim().trim_end_matches(';')))
        .unwrap_or_else(|| panic!("no Env interface:\n{compose}"))
        .collect();
    let handlers = out.text("workers/ops-hub/handlers.ts");
    let deps_env: Vec<&str> = handlers
        .split_once("env: { ")
        .and_then(|(_, rest)| rest.split_once(" }"))
        .map(|(members, _)| members.split("; "))
        .unwrap_or_else(|| panic!("no deps.env type:\n{handlers}"))
        .collect();
    assert_eq!(
        deps_env,
        [
            "DEMO_PAYMENT: __ServiceBinding",
            "LEDGER: __DurableObjectNamespace"
        ],
        "{handlers}"
    );
    for member in &deps_env {
        assert!(env.contains(member), "`{member}` is not in Env: {env:?}");
    }
}

#[test]
fn a_user_type_in_a_wrapper_signature_is_qualified_through_handlers() {
    let src = "context demo.shop

type Order = { id: String }

service place {
  on call(order: Order) -> Effect[Option[Order]] {
    Effect.pure(Some(order))
  }
}
";
    let ts = compose(&[("demo/shop.bynk", src)], "demo-shop");
    assert!(ts.contains("async place(order: handlers.Order) {"), "{ts}");
}

#[test]
fn each_http_route_gets_a_wrapper_named_by_method_and_path() {
    let src = "context demo.shop

service api from http {
  on GET(\"/items/:id\") (id: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(id)
  }
}
";
    let ts = compose(&[("demo/shop.bynk", src)], "demo-shop");
    assert!(
            ts.contains("async http_GET_items_Param_id(id: string) {\n      return handlers.api.http_GET_items_Param_id(id, deps);"),
            "{ts}"
        );
}

#[test]
fn a_provided_capability_is_constructed_once_and_passed_in_deps() {
    let src = "context demo.shop

capability Clock {
  fn now() -> Effect[Int]
}

provides Clock = FixedClock {
  fn now() -> Effect[Int] {
    Effect.pure(0)
  }
}

service probe {
  on call() -> Effect[Int] given Clock {
    Clock.now()
  }
}
";
    let ts = compose(&[("demo/shop.bynk", src)], "demo-shop");
    assert!(
        ts.contains("const Clock = new handlers.FixedClock();"),
        "{ts}"
    );
    assert_eq!(ts.matches("new handlers.FixedClock()").count(), 1, "{ts}");
    assert!(ts.contains("const deps = { Clock };"), "{ts}");
}

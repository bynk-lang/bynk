//! #2355 follow-up: a capability whose provider constructs its own context's
//! agents can't be required from another context on the Workers target
//! (`bynk.capability.provider_agent_cross_context`). The consumer's Worker
//! would build the provider with its own `env`, which doesn't bind the agents'
//! Durable Objects. On the bundle target every context shares one process, so
//! the layout still compiles. This pins the "Workers-only" claim in the
//! diagnostic's note. It also pins the paths a consumer can reach the provider
//! by: a handler's `given`, and a local provider's `given`. The Workers
//! rejection through a handler alone is covered by the negative fixture
//! `2355_workers_provider_agent_cross_context`.

use std::sync::atomic::{AtomicU32, Ordering};

const REGISTRY: &str = "context registry

exports capability { Yanks }

agent PackageIndex {
  key name: String
  store yanked: Cell[Bool]

  on call setYanked(value: Bool) -> Effect[Bool] {
    let changed = yanked != value
    yanked := value
    Effect.pure(changed)
  }
}

capability Yanks { fn set(value: Bool) -> Effect[Bool] }

provides Yanks = AgentYanks {
  fn set(value: Bool) -> Effect[Bool] {
    PackageIndex(\"x\").setYanked(value)
  }
}
";

/// `api` requires `registry.Yanks` from a handler.
const API_HANDLER: &str = "context api

consumes registry

service yank {
  on call() -> Effect[Bool] given registry.Yanks {
    registry.Yanks.set(true)
  }
}
";

/// `api` requires `registry.Yanks` only through a local provider's `given`.
const API_PROVIDER: &str = "context api

consumes registry

capability Moderation { fn hide() -> Effect[Bool] }

provides Moderation = YankModeration given registry.Yanks {
  fn hide() -> Effect[Bool] {
    registry.Yanks.set(true)
  }
}

service yank {
  on call() -> Effect[Bool] given Moderation {
    Moderation.hide()
  }
}
";

const SCHEMA_LOCK: &str = r#"version = 2

[events]

[agents."registry.PackageIndex"]
version = 1
floor = 1

[[agents."registry.PackageIndex".shapes]]
version = 1
additive = false

[[agents."registry.PackageIndex".shapes.fields]]
name = "yanked"
kind = "Cell"
type = "Bool"
refinement = "Bool"
"#;

/// Compile `registry` plus the given `api` for `target`, returning the error
/// categories.
fn compile_categories(api: &str, target: bynkc::BuildTarget) -> Result<(), Vec<String>> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "bynk_provider_agent_cross_context_{}_{unique}",
        std::process::id()
    ));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("registry.bynk"), REGISTRY).unwrap();
    std::fs::write(src.join("api.bynk"), api).unwrap();
    std::fs::write(src.join("bynk.schema.lock"), SCHEMA_LOCK).unwrap();
    let result =
        bynkc::compile_project(&bynk_testkit::compile_options_single(src.clone()).target(target))
            .map(|_| ())
            .map_err(|failure| {
                bynkc::ProjectFailure::flatten(failure)
                    .into_iter()
                    .map(|e| e.category.to_string())
                    .collect()
            });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[test]
fn a_cross_context_agent_provider_compiles_on_bundle_and_is_rejected_on_workers() {
    for (path, api) in [("handler", API_HANDLER), ("provider", API_PROVIDER)] {
        compile_categories(api, bynkc::BuildTarget::Bundle).unwrap_or_else(|errs| {
            panic!("the bundle target should accept the {path} layout: {errs:?}")
        });

        let errs = compile_categories(api, bynkc::BuildTarget::Workers)
            .expect_err("the Workers target should reject the layout");
        assert_eq!(
            errs,
            vec!["bynk.capability.provider_agent_cross_context".to_string()],
            "exactly the cross-context diagnostic, through a {path}'s `given`"
        );
    }
}

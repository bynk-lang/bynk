//! v0.166 (#616, ADR 0191): every `SymbolKind` the index resolves has a hover
//! renderer — mechanically, not by inspection.
//!
//! Hover's rung 1 resolves an offset through the project index and then *guards*
//! on the renderer returning `Some`. A resolved key with no arm therefore falls
//! through to the later, name-matching rungs — silently, and often to a
//! confidently wrong answer rather than to nothing. That has now shipped twice:
//! #611 gap B (a record `Field` label rendered a same-named handler param), and
//! #616 (`Method` rendered whichever same-named method was declared first;
//! `CapabilityOp` rendered the embedded stdlib op; `Actor` rendered nothing).
//!
//! Both times the arm was missing and nothing failed. This is the tooth ADR 0191
//! names in its Consequences, fitted rather than deferred to a third occurrence.
//! It has two halves, and needs both:
//!
//! - [`every_indexed_symbol_has_a_renderer`] is the **invariant**: sweep every
//!   key the real index produces for a fixture declaring every kind, and fail
//!   on any the renderer answers `None` for. This is what would have caught #611
//!   gap B and all three of #616.
//! - [`declared_name`]'s `match` is the **forcing function**: it is exhaustive
//!   over `SymbolKind`, so adding a variant stops this crate compiling until
//!   someone names a declaration for it. Without that, a new kind the fixture
//!   never declares would leave the sweep quietly vacuous — passing while
//!   covering nothing, which is the failure mode the sweep exists to prevent.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use bynk_check::index::SymbolKind;
use bynk_lsp::symbols;

/// One unit declaring every `SymbolKind` the index can key. It must analyse
/// **cleanly** — a diagnostic here would empty the index and pass the sweep by
/// covering nothing.
const EVERY_KIND: &str = r#"context demo.every

type Title = String where NonEmpty

type Stored = {
  title: Title,
}

fn free(n: Int) -> Int { n }

asset logo = "assets/logo.svg"

fn logoUrl() -> String { logo.url }

embed banner = "embeds/banner.txt"

fn bannerText() -> String { banner }

const MaxTodos: Int = 10

fn limit() -> Int { MaxTodos }

fn Stored.retitle(self, t: Title) -> Stored {
  Stored { title: t }
}

capability Logger { fn info(message: String) -> Effect[()] }

provides Logger = ConsoleLogger {
  fn info(message: String) -> Effect[()] {
    Effect.pure(())
  }
}

actor User { auth = None }

agent Todos {
  key owner: Title

  store count: Cell[Int]

  on call bump() -> Effect[Int] {
    let next = count + 1
    count := next
    next
  }
}

service api {
  on call() -> Effect[()] given Logger {
    let _ <- Logger.info("ping")
    Effect.pure(())
  }
}
"#;

/// message-bundles slice 1 (#859): `messages` is commons-only
/// (`bynk.messages.outside_commons` otherwise), so it can't live inside
/// [`EVERY_KIND`]'s single `context demo.every` unit — a second unit, written
/// to its own file by [`analysed`].
const MESSAGES_BUNDLE: &str = r#"commons app.bundle

uses bynk.locale
uses bynk.locale.types

messages "en" @reference {
  "greeting" => "Hi"
}
"#;

/// The name [`EVERY_KIND`] declares for `kind`.
///
/// **This `match` is the tooth.** It is exhaustive over `SymbolKind`, so a new
/// variant breaks the build here. Fixing the break means declaring one in
/// [`EVERY_KIND`], which puts the kind into the index, which puts it under the
/// sweep below — where it fails until `describe_item` has an arm for it. A
/// non-exhaustive lookup (a map, a `_` arm) would let a new kind slip through
/// exactly as `Method`/`CapabilityOp`/`Actor` did.
fn declared_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Type => "Stored",
        SymbolKind::Fn => "free",
        SymbolKind::Capability => "Logger",
        SymbolKind::Service => "api",
        SymbolKind::Agent => "Todos",
        SymbolKind::Provider => "ConsoleLogger",
        // The compound-key kinds (ADR 0069) — `"Type.method"`, `"Type.field"`,
        // `"Cap.op"`. These are the ones that shipped wrong.
        SymbolKind::Method => "Stored.retitle",
        SymbolKind::Field => "Stored.title",
        SymbolKind::CapabilityOp => "Logger.info",
        SymbolKind::Actor => "User",
        // #304: an agent handler, keyed `"Agent.handler"`.
        SymbolKind::Handler => "Todos.bump",
        // message-bundles slice 1 (#859): a messages block, keyed by its tag.
        SymbolKind::Messages => "en",
        // #1883: an asset, keyed by its name.
        SymbolKind::Asset => "logo",
        // #2387: an embed, keyed by its name.
        SymbolKind::Embed => "banner",
        // #2617: a constant, keyed by its name.
        SymbolKind::Const => "MaxTodos",
    }
}

/// Every variant, so the fixture's coverage is asserted rather than assumed.
/// `declared_name`'s exhaustive `match` is what forces this list to be extended
/// in practice: a new kind fails to compile there first.
const ALL_KINDS: [SymbolKind; 15] = [
    SymbolKind::Type,
    SymbolKind::Fn,
    SymbolKind::Capability,
    SymbolKind::Service,
    SymbolKind::Agent,
    SymbolKind::Provider,
    SymbolKind::Method,
    SymbolKind::Field,
    SymbolKind::CapabilityOp,
    SymbolKind::Actor,
    SymbolKind::Handler,
    SymbolKind::Messages,
    SymbolKind::Asset,
    SymbolKind::Embed,
    SymbolKind::Const,
];

/// [`EVERY_KIND`] analysed under its own root. `test_name` is what keeps the
/// root per-test: the harness runs both tests below on parallel threads, and
/// each tears its root down on the way in and out, so a root keyed on the pid
/// alone has one test deleting the directory the other is still writing into.
fn analysed(test_name: &str) -> (bynk_ide::ProjectDiagnostics, HashMap<PathBuf, String>) {
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-renderer-kind-coverage-{test_name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    // The unit is `context demo.every`, so its file must live at `demo/every.bynk`
    // for its path to match its declared name — otherwise
    // `bynk.project.inconsistent_commons_name` fires (now that project-level
    // diagnostics are attributed to their file and surface here, #696).
    let unit_dir = root.join("demo");
    fs::create_dir_all(&unit_dir).expect("create test root");
    fs::write(unit_dir.join("every.bynk"), EVERY_KIND).expect("write fixture");
    // `commons app.bundle` → `app/bundle.bynk`, same name/path convention.
    let bundle_dir = root.join("app");
    fs::create_dir_all(&bundle_dir).expect("create test root");
    fs::write(bundle_dir.join("bundle.bynk"), MESSAGES_BUNDLE).expect("write fixture");
    // #1883: the asset's file, else `bynk.asset.missing_file` fires.
    fs::create_dir_all(root.join("assets")).expect("create test root");
    fs::write(root.join("assets/logo.svg"), "<svg/>").expect("write fixture");
    // #2387: the embed's file, else `bynk.embed.missing_file` fires.
    fs::create_dir_all(root.join("embeds")).expect("create test root");
    fs::write(root.join("embeds/banner.txt"), "hello").expect("write fixture");
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let r = bynk_ide::diagnose_project(&root, &sources);
    let _ = fs::remove_dir_all(&root);

    let errors: Vec<_> = r
        .files
        .iter()
        .flat_map(|f| f.diagnostics.iter())
        .map(|d| d.error.category)
        .collect();
    assert!(
        errors.is_empty(),
        "the all-kinds fixture must analyse cleanly, else the index is empty \
         and the sweep covers nothing: {errors:?}"
    );

    let texts: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect();
    (r, texts)
}

/// The invariant: a key the index resolves is a key hover can render.
///
/// Sweeps the **real** index rather than a list, so it covers whatever the
/// fixture actually produces — including any kind a future change starts keying.
#[test]
fn every_indexed_symbol_has_a_renderer() {
    let (r, texts) = analysed("every_indexed_symbol_has_a_renderer");
    assert!(!r.index.symbols.is_empty(), "the index is populated");

    let mut unrendered: Vec<String> = Vec::new();
    for (key, entry) in &r.index.symbols {
        let Some(def) = &entry.def else { continue };
        let text = texts
            .iter()
            .find(|(p, _)| p.file_name() == def.path.file_name())
            .map(|(_, t)| t.as_str())
            .unwrap_or_default();
        if symbols::describe_symbol(text, &key.name).is_none() {
            unrendered.push(format!("{:?} {:?}", key.kind, key.name));
        }
    }
    unrendered.sort();
    assert!(
        unrendered.is_empty(),
        "these index keys resolve but render nothing, so hover falls through to \
         a name match and answers with a guess — give `describe_item` an arm for \
         each (ADR 0191 D1): {unrendered:?}"
    );
}

/// Keeps the sweep honest: the fixture declares every kind, so the sweep above
/// is covering all ten rather than passing on a subset.
#[test]
fn the_fixture_declares_every_symbol_kind() {
    let (r, _) = analysed("the_fixture_declares_every_symbol_kind");
    let found: HashSet<(SymbolKind, &str)> = r
        .index
        .symbols
        .keys()
        .map(|k| (k.kind, k.name.as_str()))
        .collect();

    let missing: Vec<String> = ALL_KINDS
        .iter()
        .filter(|k| !found.contains(&(**k, declared_name(**k))))
        .map(|k| format!("{:?} (expected {:?})", k, declared_name(*k)))
        .collect();
    assert!(
        missing.is_empty(),
        "`EVERY_KIND` must declare one of every kind, or the renderer sweep is \
         vacuous for the ones it misses: {missing:?}"
    );
}

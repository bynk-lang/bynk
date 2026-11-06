//! v0.177 (#643): standing guards on the cross-context contract hash.
//!
//! The hash's danger is not that it fails to catch skew — it is that it fires
//! when there *is* no skew. A spurious 409 breaks a working deployment and
//! destroys trust in the mechanism, which is worse than having no check at all.
//! Every plausible cause of one is a canonicalisation bug, so these guards test
//! that side hardest.

use std::collections::HashMap;
use std::path::Path;

/// Every caller's stamped hash equals the constant its callee compares against.
///
/// This is the no-false-positive property, checked over the whole blessed
/// corpus rather than argued: caller and callee canonicalise the callee's
/// contract from tables built by the same `combined_types_for`, so on a single
/// build they must agree for *every* service in *every* fixture. A single
/// mismatch here means the emitter ships a boundary that 409s on first contact.
#[test]
fn every_stamped_hash_matches_its_callees_constant() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let stamp =
        // Capture the binding (whose worker is called), the service, and the
        // stamped hash.
        regex::Regex::new(
            r#"callService\(deps\.env\.([A-Z0-9_]+), "([A-Za-z_][A-Za-z0-9_]*)",.*?"([0-9a-f]{16})", deps\.env, "[a-z0-9_.]+"\)"#,
        )
            .unwrap();
    let expect = regex::Regex::new(
        r#"kind: "ContractMismatch", service: "([^"]+)", expected: "([0-9a-f]{16})""#,
    )
    .unwrap();

    // Keyed by (fixture, **worker**, service): two contexts in one fixture may
    // each provide a same-named service, and a (fixture, service) key would
    // collide — last write wins, giving a false pass or a false failure.
    let mut expected: HashMap<(String, String, String), String> = HashMap::new();
    // A caller records which worker it *targets*, since that is whose constant
    // it must match.
    let mut stamped: Vec<(String, String, String, String)> = Vec::new();

    let Ok(entries) = std::fs::read_dir(&root) else {
        panic!("fixture root not readable: {}", root.display());
    };
    for e in entries.flatten() {
        let fx = e.file_name().to_string_lossy().to_string();
        let workers = e.path().join("expected/workers");
        let Ok(dirs) = std::fs::read_dir(&workers) else {
            continue;
        };
        for d in dirs.flatten() {
            let worker = d.file_name().to_string_lossy().to_string();
            if let Ok(s) = std::fs::read_to_string(d.path().join("index.ts")) {
                for c in expect.captures_iter(&s) {
                    expected.insert(
                        (fx.clone(), worker.clone(), c[1].to_string()),
                        c[2].to_string(),
                    );
                }
            }
        }
        // Callers live in a Worker's `handlers.ts` *and* in the emitted
        // integration tests, which route through the same
        // `lower_workers_cross_context_call`. Scanning only the former would
        // leave a whole class of stamped call site unchecked here; they would
        // fail loudly under workerd, but this guard is the cheap one.
        for f in caller_sources(&e.path()) {
            let Ok(s) = std::fs::read_to_string(&f) else {
                continue;
            };
            for c in stamp.captures_iter(&s) {
                stamped.push((
                    fx.clone(),
                    c[1].to_string(),
                    c[2].to_string(),
                    c[3].to_string(),
                ));
            }
        }
    }

    assert!(
        !stamped.is_empty(),
        "no cross-context call sites found — the guard would pass vacuously"
    );

    let mut problems: Vec<String> = Vec::new();
    for (fx, binding, svc, h) in &stamped {
        // `deps.env.SHOP_PAYMENT` → the `shop-payment` worker directory.
        let worker = binding.to_ascii_lowercase().replace('_', "-");
        match expected.get(&(fx.clone(), worker.clone(), svc.clone())) {
            None => problems.push(format!(
                "{fx}: caller stamps {h} for {worker}.{svc}, but that worker emits no contract check"
            )),
            Some(e) if e != h => problems.push(format!(
                "{fx}: caller stamps {h} for {worker}.{svc} but the callee expects {e} — \
                 a spurious 409 on every call"
            )),
            Some(_) => {}
        }
    }
    assert!(
        problems.is_empty(),
        "contract hashes disagree within a single build:\n{}",
        problems.join("\n")
    );
}

/// Compile a two-context workers project and return the hash `a` stamps for
/// `b.probe`, read out of the emitted caller.
fn stamped_hash(b_body: &str, tag: &str) -> String {
    use bynk_driver::project::BuildTarget;
    let tmp = std::env::temp_dir().join(format!("bynk-contract-nf-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    // Flat layout (no `src/`), matching `cross_context_caller.rs`: the project
    // root *is* the include root, so `app/b.bynk` declares `app.b`.
    let proj = tmp.join("proj/app");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("b.bynk"), b_body).unwrap();
    std::fs::write(
        proj.join("a.bynk"),
        "context app.a\n\nconsumes app.b as B\n\nservice ask {\n  on call(x: String) -> Effect[Result[String, String]] {\n    let r <- B.probe(x)\n    r\n  }\n}\n",
    )
    .unwrap();
    let out = bynk_driver::project::compile_project(
        &bynk_testkit::compile_options_single(tmp.join("proj")).target(BuildTarget::Workers),
    )
    .unwrap_or_else(|f| {
        panic!(
            "compile {tag}:\n{}",
            bynk_driver::render::render_project_errors(&f.flatten())
        )
    });
    let caller_text = out
        .artefacts
        .docs
        .iter()
        .find(|(p, _)| p.ends_with("workers/app-a/handlers.ts"))
        .expect("caller emitted")
        .1
        .text();
    let h = regex::Regex::new(r#"callService\([^)]*?"([0-9a-f]{16})", deps\.env, "[a-z0-9_.]+"\)"#)
        .unwrap()
        .captures(&caller_text)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| panic!("no stamped hash in {tag}:\n{caller_text}"));
    let _ = std::fs::remove_dir_all(&tmp);
    h
}

/// A **parameter rename** is a wire change, not a refactor: the multi-argument
/// wire is an object keyed by parameter name.
///
/// End-to-end rather than over the normal form directly, because it is one of
/// the few contract changes a *co-compiled* pair does not already reject: the
/// caller passes positionally, so only the callee's spelling moves. Most other
/// changes (a renamed record field, a retyped parameter) are caught by the
/// checker's structural rule long before a hash could speak — which is the point
/// of the hash living where it does. It exists for the pair that was *never*
/// co-compiled: A built at rev1, B redeployed at rev2. Those cases are covered
/// over the normal form itself, in `bynk-check`'s `contract` unit tests, since no
/// single build can express them.
#[test]
fn a_parameter_rename_changes_the_hash() {
    let a = stamped_hash(
        "context app.b\n\nservice probe {\n  on call(x: String) -> Effect[Result[String, String]] {\n    Ok(x)\n  }\n}\n",
        "param-x",
    );
    // The caller calls `B.probe(x)` positionally, so only the callee's parameter
    // name differs — which is exactly the case a name-blind form would miss.
    let b = stamped_hash(
        "context app.b\n\nservice probe {\n  on call(renamed: String) -> Effect[Result[String, String]] {\n    Ok(renamed)\n  }\n}\n",
        "param-renamed",
    );
    assert_ne!(a, b, "a parameter rename must change the hash");
}

/// Every emitted file that may contain a stamped cross-context call: each
/// Worker's `handlers.ts`, plus the emitted integration tests, which lower
/// through the same path.
fn caller_sources(fixture: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(dirs) = std::fs::read_dir(fixture.join("expected/workers")) {
        for d in dirs.flatten() {
            out.push(d.path().join("handlers.ts"));
        }
    }
    if let Ok(files) = std::fs::read_dir(fixture.join("expected/tests")) {
        for f in files.flatten() {
            out.push(f.path());
        }
    }
    out
}

/// #2750: a Worker's `bynk-contracts.json` `provides` hash is the hash its own
/// `index.ts` enforces. Both are computed from one type table
/// (`contract_types_for`); a second, narrower table once left a consumed
/// context's exported type unresolved in the manifest, so a driver reading the
/// manifest got a `409 ContractMismatch`. Swept over every blessed workers
/// fixture, so any new divergence between the two sites fails here.
#[test]
fn manifest_provides_equals_the_hash_the_entry_enforces() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/positive");
    let expect = regex::Regex::new(
        r#"kind: "ContractMismatch", service: "([^"]+)", expected: "([0-9a-f]{16})""#,
    )
    .unwrap();
    let mut checked = 0usize;
    let mut problems: Vec<String> = Vec::new();
    for e in std::fs::read_dir(&root).unwrap().flatten() {
        let fx = e.file_name().to_string_lossy().to_string();
        let Ok(dirs) = std::fs::read_dir(e.path().join("expected/workers")) else {
            continue;
        };
        for d in dirs.flatten() {
            let Ok(manifest) = std::fs::read_to_string(d.path().join("bynk-contracts.json")) else {
                continue;
            };
            let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
            let Some(provides) = manifest["provides"].as_object() else {
                continue;
            };
            let entry = std::fs::read_to_string(d.path().join("index.ts")).unwrap_or_default();
            let enforced: HashMap<String, String> = expect
                .captures_iter(&entry)
                .map(|c| (c[1].to_string(), c[2].to_string()))
                .collect();
            let worker = d.file_name().to_string_lossy().to_string();
            for (svc, h) in provides {
                checked += 1;
                // A foreign caller's recorded hash is the one the entry enforces.
                if let Some(actors) = manifest["foreign"].as_object() {
                    for (actor, svcs) in actors {
                        if let Some(fh) = svcs.get(svc).filter(|fh| *fh != h) {
                            problems.push(format!(
                                "{fx}/{worker}: foreign {actor}.{svc}={fh} differs from provides {h}"
                            ));
                        }
                    }
                }
                if enforced.get(svc).map(String::as_str) != h.as_str() {
                    problems.push(format!(
                        "{fx}/{worker}: manifest provides {svc}={h}, index.ts enforces {:?}",
                        enforced.get(svc)
                    ));
                }
            }
        }
    }
    assert!(checked > 0, "no provides entries found — vacuous");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

//! #3959 (capability-parameters track §13, slice 4): over real `diagnose_project` output, hover on
//! a call that binds a commons function's capability parameters names the instance each one is
//! bound to, the function's signature shows each parameter's bound, and hover on a binding's
//! parameter name says what it binds.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use bynk_lsp::hover;

const LIB: &str = "commons lib.cache\n\
\n\
consumes bynk.cloudflare { Kv }\n\
\n\
fn remember(key: String) -> Effect[()] given Store: Kv {\n\
\tStore.put(key, \"v\")\n\
}\n";

const SHOP: &str = "context shop\n\
\n\
consumes bynk.cloudflare { Kv }\n\
\n\
uses lib.cache\n\
\n\
capability Sessions = Kv\n\
\n\
service api {\n\
\ton call(key: String) -> Effect[()] by Caller given Sessions {\n\
\t\tremember(key, given Store = Sessions)\n\
\t}\n\
}\n";

struct Fixture {
    top: PathBuf,
    result: bynk_ide::ProjectDiagnostics,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.top);
    }
}

fn fixture(tag: &str) -> Fixture {
    let top = std::env::temp_dir().join(format!("bynk_lsp_cap_param_{}_{tag}", std::process::id()));
    let root = top.join("src");
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::write(root.join("lib").join("cache.bynk"), LIB).unwrap();
    fs::write(root.join("shop.bynk"), SHOP).unwrap();
    fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n\n[unstable]\nallow = [\"bynk.given.parameters\"]\n",
    )
    .unwrap();
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    Fixture { top, result }
}

/// The project-relative key of the analysed file whose path ends with `name`.
fn rel_of(r: &bynk_ide::ProjectDiagnostics, name: &str) -> PathBuf {
    let file = r
        .files
        .iter()
        .find(|x| x.source_path.ends_with(name))
        .unwrap_or_else(|| panic!("{name} analysed"));
    r.locals
        .keys()
        .find(|p| p.ends_with(name))
        .cloned()
        .unwrap_or_else(|| file.source_path.clone())
}

fn hover_in_shop(f: &Fixture, offset: usize) -> Option<String> {
    let r = &f.result;
    let snapshots: HashMap<PathBuf, String> = [("cache.bynk", LIB), ("shop.bynk", SHOP)]
        .into_iter()
        .map(|(name, text)| (rel_of(r, name), text.to_string()))
        .collect();
    let rel = rel_of(r, "shop.bynk");
    let root = f.top.join("src");
    hover::hover_content(&hover::HoverInput {
        analysis: Some(hover::HoverAnalysis {
            index: &r.index,
            snapshots: &snapshots,
            locals: &r.locals,
            expr_types: &r.expr_types,
            tys: &r.ty_intern,
            rel: &rel,
            offset,
            capability_paths: &r.capability_paths,
            project_root: &root,
            doc_scope: &r.doc_scope,
            boundary_info: &r.boundary_info,
            context_count: bynk_ide::wire_contract::real_context_count(
                &r.boundary_info,
                &r.unit_sources,
            ),
            analytics: &r.analytics,
        }),
        doc: Some((SHOP, offset)),
        path: None,
        files: None,
    })
}

/// The fixture compiles clean, so the hovers below read a real analysis.
fn assert_clean(f: &Fixture) {
    let errors: Vec<_> = f
        .result
        .files
        .iter()
        .flat_map(|x| &x.diagnostics)
        .filter(|d| d.severity == bynk_syntax::error::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn hover_on_a_binding_call_names_the_bound_instance() {
    let f = fixture("call");
    assert_clean(&f);
    let at = SHOP.find("remember(").unwrap() + 3;
    let md = hover_in_shop(&f, at).expect("a hover");
    assert!(md.contains("given Store: Kv"), "{md}");
    assert!(md.contains("`Store` is bound to `Sessions`"), "{md}");
}

#[test]
fn hover_on_a_binding_parameter_says_what_it_binds() {
    let f = fixture("param");
    let at = SHOP.find("Store =").unwrap() + 2;
    let md = hover_in_shop(&f, at).expect("a hover");
    assert!(md.contains("a capability parameter of `remember`"), "{md}");
    assert!(md.contains("bound to `Sessions`"), "{md}");
}

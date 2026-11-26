//! #4502 / #4451: the editor's answers about the typed Analytics operations, over real
//! `diagnose_project` output: hover on a written field names its column, hover on `{dataset}` names
//! the table, and the dataset placeholder gets its own semantic token span.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use bynk_lsp::hover;

const SRC: &str = "context shop.cart\n\
\n\
consumes bynk.cloudflare { Analytics, AnalyticsSql }\n\
\n\
type Signup = { @index plan: String, country: String, seats: Int }\n\
\n\
capability Signups = Analytics\n\
\n\
service api from http {\n\
\ton GET(\"/x\") () -> Effect[HttpResult[Int]] by Visitor given AnalyticsSql, Signups {\n\
\t\tlet _ <- Signups.writeValue(Signup { plan: \"pro\", country: \"GB\", seats: 3 })\n\
\t\tlet _ <- AnalyticsSql.query(Signups, \"SELECT count() FROM {dataset}\", [])\n\
\t\tOk(1)\n\
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
    let top = std::env::temp_dir().join(format!("bynk_lsp_analytics_{}_{tag}", std::process::id()));
    let root = top.join("src");
    fs::create_dir_all(root.join("shop")).unwrap();
    fs::write(root.join("shop").join("cart.bynk"), SRC).unwrap();
    fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n\n[unstable]\nallow = [\"bynk.cloudflare.Analytics\", \"bynk.cloudflare.AnalyticsSql\"]\n",
    )
    .unwrap();
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    Fixture { top, result }
}

fn hover_at(f: &Fixture, offset: usize) -> Option<String> {
    let r = &f.result;
    let file = r
        .files
        .iter()
        .find(|x| x.source_path.ends_with("cart.bynk"))
        .expect("cart.bynk analysed");
    let rel: PathBuf = r
        .locals
        .keys()
        .find(|p| p.ends_with("cart.bynk"))
        .cloned()
        .unwrap_or_else(|| file.source_path.clone());
    let snapshots: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|x| (rel.clone(), x.text.clone()))
        .collect();
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
        doc: Some((SRC, offset)),
        path: None,
        files: None,
    })
}

#[test]
fn the_round_records_the_written_mapping() {
    let f = fixture("mapping");
    let entry = f
        .result
        .analytics
        .get("shop.cart.Signups")
        .unwrap_or_else(|| {
            panic!(
                "no mapping: {:?}; {:?}",
                f.result.analytics,
                f.result
                    .files
                    .iter()
                    .map(|x| &x.diagnostics)
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(entry.record, "Signup");
    assert_eq!(entry.index.as_deref(), Some("plan"));
}

#[test]
fn hover_on_a_written_field_names_its_column() {
    let f = fixture("column");
    let at = SRC.find("country: String").unwrap() + 2;
    let md = hover_at(&f, at).expect("a hover");
    assert!(md.contains("Signups"), "{md}");
    assert!(md.contains("`blob1` (text)"), "{md}");
    let at = SRC.find("seats: Int").unwrap() + 2;
    let md = hover_at(&f, at).expect("a hover");
    assert!(md.contains("`double1` (number)"), "{md}");
}

#[test]
fn hover_on_the_dataset_placeholder_names_the_table() {
    let f = fixture("dataset");
    let at = SRC.find("{dataset}").unwrap() + 3;
    let md = hover_at(&f, at).expect("a hover");
    assert!(
        md.contains("events.analyticsEngine.shop_cart_signups"),
        "{md}"
    );
}

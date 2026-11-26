//! #4099: hovering `Ai.generateValue[T]` shows the JSON Schema the call sends the model, over real
//! `diagnose_project` output.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use bynk_lsp::hover;

const SRC: &str = "context shop.triage\n\
\n\
consumes bynk.cloudflare { Ai, GenerateRequest }\n\
\n\
type Severity = enum { Low, High }\n\
type Triage = { severity: Severity, summary: String where MaxLength(200) }\n\
\n\
service api from http {\n\
\t@limit(maxBody: 64)\n\
\ton POST(\"/t\") (body: String) -> Effect[HttpResult[Int]] by Visitor given Ai {\n\
\t\tlet r <- Ai.generateValue[Triage](\"m\", GenerateRequest { messages: [] })\n\
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
    let top = std::env::temp_dir().join(format!("bynk_lsp_ai_value_{}_{tag}", std::process::id()));
    let root = top.join("src");
    fs::create_dir_all(root.join("shop")).unwrap();
    fs::write(root.join("shop").join("triage.bynk"), SRC).unwrap();
    fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n\n[unstable]\nallow = [\"bynk.cloudflare.Ai\", \"bynk.cloudflare.Ai.generateValue\"]\n",
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
        .find(|x| x.source_path.ends_with("triage.bynk"))
        .expect("triage.bynk analysed");
    let rel: PathBuf = r
        .locals
        .keys()
        .find(|p| p.ends_with("triage.bynk"))
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
fn the_call_hovers_with_the_schema_it_sends() {
    let f = fixture("schema");
    let at = SRC.find("generateValue").unwrap() + 2;
    let md = hover_at(&f, at).expect("a hover");
    assert!(md.contains("generateValue[T](model: String"), "{md}");
    assert!(md.contains("**Sent to the model**"), "{md}");
    assert!(md.contains("\"maxLength\": 200"), "{md}");
    assert!(md.contains("\"enum\": ["), "{md}");
}

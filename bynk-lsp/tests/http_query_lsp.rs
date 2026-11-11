//! #2854: hovering an `on http` handler's typed `query` parameter lists the keys
//! it reads off the query string, with their types and defaults.

use bynk_lsp::hover;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::Url;

const SRC: &str = "\
context demo.activity

type Limit = Int where InRange(1, 500)

type Filter = {
  owner: Option[String],
  tag: List[String] = [],
  limit: Limit = 50,
  page: Int,
}

type Prefs = {
  sid: Option[String],
  theme: String = \"light\",
}

service api from http {
  on GET(\"/prefs\") (cookies: Prefs) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(cookies.theme)
  }

  on GET(\"/activity\") (query: Filter) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(\"\\(query.limit)\")
  }

  on GET(\"/echo\") (query: RequestQuery) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(query.get(\"q\").getOrElse(\"-\"))
  }
}
";

const PATH: &str = "demo/activity.bynk";

fn offset_of(needle: &str, n: usize) -> usize {
    SRC.match_indices(needle).nth(n).expect("needle present").0
}

fn analyse() -> (bynk_ide::ProjectDiagnostics, PathBuf) {
    // Both tests run in one process under `cargo test`, so the pid alone names a
    // directory they would share and delete from under each other.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "bynk-lsp-http-query-{}-{unique}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("demo")).expect("create test root");
    fs::write(root.join(PATH), SRC).expect("write file");
    let root = root.canonicalize().unwrap_or(root);
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let result = bynk_ide::diagnose_project(&root, &sources);
    let errors: Vec<String> = result
        .files
        .iter()
        .flat_map(|f| &f.diagnostics)
        .chain(&result.unattributed)
        .filter(|d| d.severity == bynk_syntax::Severity::Error)
        .map(|d| format!("{}: {}", d.error.category, d.error.message))
        .collect();
    assert!(
        errors.is_empty(),
        "the fixture should check cleanly: {errors:?}"
    );
    (result, root)
}

fn hover_at(r: &bynk_ide::ProjectDiagnostics, root: &Path, offset: usize) -> Option<String> {
    let rel = PathBuf::from(PATH);
    let snapshots: HashMap<PathBuf, String> = HashMap::from([(rel.clone(), SRC.to_string())]);
    let uri = Url::parse("file:///activity.bynk").unwrap();
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
            project_root: root,
            doc_scope: &r.doc_scope,
            boundary_info: &r.boundary_info,
            context_count: bynk_ide::wire_contract::real_context_count(
                &r.boundary_info,
                &r.unit_sources,
            ),
        }),
        doc: Some((SRC, offset)),
        path: uri.to_file_path().ok().as_deref(),
        files: None,
    })
}

#[test]
fn a_typed_query_hovers_with_its_keys_types_and_defaults() {
    let (result, root) = analyse();
    // The parameter's own name, and a use of it in the body.
    for offset in [offset_of("query: Filter", 0), offset_of("query.limit", 0)] {
        let h = hover_at(&result, &root, offset + 1).expect("a hover");
        assert!(h.contains("param query: Filter"), "{h}");
        assert!(h.contains("**Query string**"), "{h}");
        assert!(
            h.contains("| `owner` | `Option[String]` | optional |"),
            "{h}"
        );
        assert!(
            h.contains("| `tag` | `List[String]` | repeatable, default `[]` |"),
            "{h}"
        );
        assert!(h.contains("| `limit` | `Limit` | default `50` |"), "{h}");
        assert!(h.contains("| `page` | `Int` | required |"), "{h}");
    }
}

#[test]
fn the_raw_view_hovers_as_its_type_alone() {
    let (result, root) = analyse();
    let h = hover_at(&result, &root, offset_of("query: RequestQuery", 0) + 1).expect("a hover");
    assert!(h.contains("param query: RequestQuery"), "{h}");
    assert!(!h.contains("**Query string**"), "{h}");
}

#[test]
fn typed_cookies_hover_with_their_cookies_types_and_defaults() {
    let (result, root) = analyse();
    // #3277: the parameter's own name, and a use of it in the body.
    for offset in [
        offset_of("cookies: Prefs", 0),
        offset_of("cookies.theme", 0),
    ] {
        let h = hover_at(&result, &root, offset + 1).expect("a hover");
        assert!(h.contains("**Cookies**"), "{h}");
        assert!(h.contains("| `sid` | `Option[String]` | optional |"), "{h}");
        assert!(
            h.contains("| `theme` | `String` | default `\"light\"` |"),
            "{h}"
        );
    }
}

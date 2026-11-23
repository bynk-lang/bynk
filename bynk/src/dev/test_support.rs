use crate::dev::serving::Serving;
use std::collections::BTreeMap;

pub(super) fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

pub(super) fn group_project(tag: &str, manifest: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("bynk-dev-groups-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("bynk.toml"), manifest).unwrap();
    root
}

pub(super) fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// A project whose `shop` Worker binds a `Vectorize` capability, `Docs`, that
/// `[dev.vectorize-index]` binds to `docs-dev`; the generated config names the
/// deploy's derived index, `shop-docs`. `extra` is appended to the generated
/// config.
pub(super) fn vectorize_project(
    tag: &str,
    extra: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "bynk-dev-configs-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let workers = root.join("out").join("workers");
    let dir = workers.join("shop");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[dev.vectorize-index]\nDocs = \"docs-dev\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("wrangler.toml"), generated_shop_config(extra)).unwrap();
    (root, workers)
}

/// The build's model for that project: `shop` declares the `Docs` index.
pub(super) fn shop_model() -> bynk_emit::emit_model::WorkersBuild {
    use bynk_check::worker_bindings::{BindingSource, WorkerBinding};
    bynk_emit::emit_model::WorkersBuild {
        bindings: bynk_emit::emitter::WorkerBindings::of_workers(BTreeMap::from([(
            "shop".to_string(),
            vec![WorkerBinding {
                name: "VECTORIZE_DOCS".to_string(),
                source: BindingSource::VectorizeIndex {
                    capability: "Docs".to_string(),
                    dimensions: 768,
                    metric: "cosine".to_string(),
                    indexed: Vec::new(),
                },
            }],
        )])),
        layout: bynk_project::worker_layout::WorkerLayout::identity(),
    }
}

pub(super) fn generated_shop_config(extra: &str) -> String {
    format!(
        "name = \"shop\"\n\n[[vectorize]]\nbinding = \"VECTORIZE_DOCS\"\nindex_name = \"shop-docs\"\n{extra}"
    )
}

pub(super) fn serving_shop() -> Vec<Serving> {
    vec![Serving {
        worker: "shop".to_string(),
        port: None,
        inspector_port: None,
        inspect: false,
    }]
}

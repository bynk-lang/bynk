//! Gated growth probe `wrangler_resource_ops` (back-half paydown 1D.4, #4056 Decision I).
//!
//! The account-resource operations that still run as a wrangler process. `OpKind`
//! (`bynk/src/cloudflare/error.rs`) has one variant per wrangler operation, and a kind that
//! moves to Cloudflare's REST API takes its variants with it (a variant no wrangler call can
//! fail with would be dead code). So the probe counts `OpKind`'s variants, less the operations
//! Q4 and #4056 Decision B keep on wrangler ([`KEPT_ON_WRANGLER`]): the push and the rest of
//! running code, and the Worker-scoped operations.
//!
//! It read 15 on `main` before 1D.4 (KV 3, R2 1, D1 2, Vectorize 6, queues 3) and falls as each
//! kind moves: 12 after 1D.4a (KV), 9 after 1D.4b (R2, D1), 3 after 1D.4c (Vectorize) and 0
//! after 1D.4d (queues). Its row names the kept operations, so the boundary shows in the table.
//! A growth probe: a new resource operation on wrangler needs a pending file that `raises:` it
//! with a reason.

use std::path::Path;

use super::Probe;

/// The operations that stay on wrangler, by their `OpKind` names: the push, `triggers deploy`,
/// `rollback`, D1 migrations (remote and `bynk dev`'s local ones), `whoami` and `bynk dev`'s
/// local Secrets Store listing (Q4: wrangler builds and runs code), and the Worker-scoped
/// deployments listing and secrets (Decision B).
pub(crate) const KEPT_ON_WRANGLER: [&str; 11] = [
    "D1MigrationsApply",
    "D1MigrationsApplyLocal",
    "NamedSecret",
    "Push",
    "Rollback",
    "SecretList",
    "SecretPut",
    "SecretsStoreListLocal",
    "TriggersDeploy",
    "Whoami",
    "WorkerDeployments",
];

/// Where `OpKind` is declared, relative to the repo root.
const OP_KIND_FILE: &str = "bynk/src/cloudflare/error.rs";

pub(crate) fn wrangler_resource_ops(root: &Path) -> Probe {
    let source = std::fs::read_to_string(root.join(OP_KIND_FILE)).unwrap_or_default();
    Probe {
        name: "wrangler_resource_ops",
        gated: true,
        reads: reading(&source),
    }
}

/// The reading: the count, the operations it counts, and the kept list.
fn reading(source: &str) -> String {
    let Some(variants) = op_kinds(source) else {
        return format!("error: no `enum OpKind` in {OP_KIND_FILE}");
    };
    let counted: Vec<&str> = variants
        .iter()
        .copied()
        .filter(|v| !KEPT_ON_WRANGLER.contains(v))
        .collect();
    format!(
        "{} ({}); kept on wrangler: {}",
        counted.len(),
        if counted.is_empty() {
            "none".to_string()
        } else {
            counted.join(", ")
        },
        KEPT_ON_WRANGLER.join(", ")
    )
}

/// `OpKind`'s variants, in declaration order: the identifiers of the enum body's lines, comments
/// and attributes skipped.
fn op_kinds(source: &str) -> Option<Vec<&str>> {
    let start = source.find("enum OpKind {")? + "enum OpKind {".len();
    let body = &source[start..start + source[start..].find('}')?];
    Some(
        body.lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//") && !line.starts_with('#'))
            .filter_map(|line| line.strip_suffix(','))
            .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_alphanumeric()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_counts_the_variants_not_kept_on_wrangler() {
        let source = "/// Which call.\n#[derive(Debug)]\npub(crate) enum OpKind {\n    QueueCreate,\n    // a note\n    Push,\n    R2Create,\n    Whoami,\n}\n";
        assert_eq!(
            reading(source),
            format!(
                "2 (QueueCreate, R2Create); kept on wrangler: {}",
                KEPT_ON_WRANGLER.join(", ")
            )
        );
        let done = "enum OpKind {\n    Push,\n}";
        assert!(reading(done).starts_with("0 (none); "));
        assert!(reading("enum Other {}").starts_with("error: "));
    }

    #[test]
    fn the_kept_list_is_sorted_and_distinct() {
        let mut sorted = KEPT_ON_WRANGLER.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, KEPT_ON_WRANGLER);
    }
}

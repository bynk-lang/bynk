//! #4127: the `[observability]` table of a Worker's `wrangler.toml`.

use super::toml_doc::{TomlBlock, TomlEntry, TomlValue};
use bynk_project::wrangler_keys::WranglerKey;

/// #4127: the `[observability]` table Workers Logs reads: whether the Worker's
/// logs are collected, and the fraction kept. Written into every Worker's
/// `wrangler.toml`, which each named environment inherits.
pub fn observability_block(observability: bynk_project::observability::Observability) -> TomlBlock {
    TomlBlock::table(
        WranglerKey::Observability,
        vec![
            TomlEntry::kv("enabled", TomlValue::Bool(observability.enabled)),
            TomlEntry::kv(
                "head_sampling_rate",
                TomlValue::Float(observability.head_sampling_rate),
            ),
        ],
    )
}

/// #4128: the `[limits]` and `[placement]` tables of a Worker's `wrangler.toml`,
/// each only when set: a project that says nothing keeps Cloudflare's defaults.
pub fn tuning_blocks(tuning: bynk_project::worker_tuning::WorkerTuning) -> Vec<TomlBlock> {
    let mut blocks = Vec::new();
    if let Some(cpu_ms) = tuning.cpu_ms {
        blocks.push(TomlBlock::table(
            WranglerKey::Limits,
            vec![TomlEntry::kv("cpu_ms", TomlValue::Int(cpu_ms as i64))],
        ));
    }
    if tuning.placement == bynk_project::worker_tuning::Placement::Smart {
        blocks.push(TomlBlock::table(
            WranglerKey::Placement,
            vec![TomlEntry::kv("mode", TomlValue::str("smart"))],
        ));
    }
    blocks
}

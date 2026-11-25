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

//! The `[[queues.consumers]]` table of a queue consumer (#3877).

use crate::emitter::toml_doc::{TomlEntry, TomlValue};

/// One `[[queues.consumers]]` table. An unset key is not written, so Cloudflare's
/// default stands; `max_batch_size` stays 10 when `batchSize` is unset.
pub(super) fn entries(
    name: &str,
    delivery: Option<&bynk_check::symbols::QueueDelivery>,
) -> Vec<TomlEntry> {
    let int = TomlValue::Int;
    let mut entries = vec![
        TomlEntry::kv("queue", TomlValue::str(name.to_string())),
        TomlEntry::kv(
            "max_batch_size",
            int(delivery.and_then(|d| d.batch_size).unwrap_or(10)),
        ),
    ];
    let Some(d) = delivery else {
        return entries;
    };
    if let Some(n) = d.batch_wait_secs {
        entries.push(TomlEntry::kv("max_batch_timeout", int(n)));
    }
    if let Some(n) = d.retries {
        entries.push(TomlEntry::kv("max_retries", int(n)));
    }
    if let Some(n) = d.retry_delay_secs {
        entries.push(TomlEntry::kv("retry_delay", int(n)));
    }
    if let Some(dlq) = &d.dead_letter {
        entries.push(TomlEntry::kv(
            "dead_letter_queue",
            TomlValue::str(dlq.clone()),
        ));
    }
    if let Some(n) = d.concurrency {
        entries.push(TomlEntry::kv("max_concurrency", int(n)));
    }
    entries
}

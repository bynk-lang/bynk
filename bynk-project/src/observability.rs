//! #4127: Workers Logs. `Logger.log`'s structured fields are queryable on
//! Cloudflare only while the Worker's `wrangler.toml` enables observability,
//! so the toolchain writes the setting itself:
//!
//! ```toml
//! [workers.observability]
//! enabled = true               # the default
//! head_sampling_rate = 0.1     # keep 10% of invocations' logs; default 1
//!
//! [deploy.workers.payments.observability]
//! head_sampling_rate = 0.01    # a noisy Worker, overriding the project's
//! ```
//!
//! A project that says nothing gets `enabled = true` and `head_sampling_rate =
//! 1`, so the documented promise holds. A group's table overrides the project's
//! key by key.

/// The keys of an observability table.
pub const KEYS: &[&str] = &["enabled", "head_sampling_rate"];

/// A Worker's resolved observability settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observability {
    /// Whether Workers Logs collects the Worker's logs.
    pub enabled: bool,
    /// The fraction of invocations whose logs are kept, from 0 to 1.
    pub head_sampling_rate: f64,
}

// The rate is checked to lie in 0..=1, which rules out NaN, so equality is total.
impl Eq for Observability {}
impl Eq for ObservabilityOverride {}

impl Default for Observability {
    fn default() -> Self {
        Self {
            enabled: true,
            head_sampling_rate: 1.0,
        }
    }
}

/// A table's own keys, each `None` when the table does not set it: what a
/// worker group overrides, and what `[workers.observability]` is read as.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ObservabilityOverride {
    pub enabled: Option<bool>,
    pub head_sampling_rate: Option<f64>,
}

impl ObservabilityOverride {
    /// `base` with this table's keys laid over it.
    pub fn over(self, base: Observability) -> Observability {
        Observability {
            enabled: self.enabled.unwrap_or(base.enabled),
            head_sampling_rate: self.head_sampling_rate.unwrap_or(base.head_sampling_rate),
        }
    }
}

/// Read an observability table. The error is the reason, to follow the table's
/// name (`` `[workers.observability]` {reason} ``).
pub fn from_table(table: &toml::Table) -> Result<ObservabilityOverride, String> {
    if let Some(unknown) = table.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(format!(
            "has no key named `{unknown}` (its keys are `enabled` and `head_sampling_rate`)"
        ));
    }
    let enabled = match table.get("enabled") {
        None => None,
        Some(toml::Value::Boolean(b)) => Some(*b),
        Some(_) => return Err("`enabled` must be `true` or `false`".to_string()),
    };
    let head_sampling_rate = match table.get("head_sampling_rate") {
        None => None,
        Some(v) => {
            let rate = match v {
                toml::Value::Float(f) => *f,
                toml::Value::Integer(i) => *i as f64,
                _ => return Err("`head_sampling_rate` must be a number from 0 to 1".to_string()),
            };
            if !(0.0..=1.0).contains(&rate) {
                return Err(format!(
                    "`head_sampling_rate` is {rate}, but it must be from 0 to 1 (the fraction of invocations whose logs are kept)"
                ));
            }
            Some(rate)
        }
    };
    Ok(ObservabilityOverride {
        enabled,
        head_sampling_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(src: &str) -> Result<ObservabilityOverride, String> {
        from_table(&src.parse::<toml::Table>().unwrap())
    }

    #[test]
    fn the_default_is_on_and_keeps_everything() {
        assert_eq!(
            Observability::default(),
            Observability {
                enabled: true,
                head_sampling_rate: 1.0
            }
        );
    }

    #[test]
    fn false_and_a_rate_round_trip_and_an_override_lays_over_the_base() {
        let t = read("enabled = false\nhead_sampling_rate = 0.25\n").unwrap();
        assert_eq!(t.enabled, Some(false));
        assert_eq!(t.head_sampling_rate, Some(0.25));
        let group = read("head_sampling_rate = 0\n").unwrap();
        let base = Observability {
            enabled: true,
            head_sampling_rate: 0.5,
        };
        assert_eq!(
            group.over(base),
            Observability {
                enabled: true,
                head_sampling_rate: 0.0
            }
        );
    }

    #[test]
    fn an_out_of_range_rate_a_wrong_type_and_an_unknown_key_are_refused() {
        assert!(read("head_sampling_rate = 1.5\n").unwrap_err().contains("0 to 1"));
        assert!(read("head_sampling_rate = -0.1\n").is_err());
        assert!(read("head_sampling_rate = \"half\"\n").is_err());
        assert!(read("enabled = \"yes\"\n").is_err());
        assert!(read("sampling = 1\n").unwrap_err().contains("no key named `sampling`"));
    }
}

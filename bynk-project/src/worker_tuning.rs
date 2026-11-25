//! #4128: a Worker's CPU limit and Smart Placement, from `bynk.toml`:
//!
//! ```toml
//! [workers]
//! cpu_ms = 50
//! placement = "smart"
//!
//! [deploy.workers.auth]
//! cpu_ms = 2000           # password hashing
//! ```
//!
//! `cpu_ms` becomes `[limits] cpu_ms` and `placement = "smart"` becomes
//! `[placement] mode = "smart"` in each generated `wrangler.toml`; both are left
//! out when unset, so Cloudflare's defaults apply. A group's keys override the
//! project's, key by key, and a named environment inherits them.

/// The CPU time a Worker may be given, in milliseconds: the platform's
/// documented maximum (five minutes). A limit above what the account's plan
/// allows is the platform's own error at deploy.
pub const MAX_CPU_MS: u64 = 300_000;

/// The keys `[workers]` and a worker group take for this.
pub const KEYS: &[&str] = &["cpu_ms", "placement"];

/// Where Cloudflare runs a Worker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Placement {
    /// Near the user: Cloudflare's default.
    #[default]
    Off,
    /// Smart Placement: near the back end the Worker talks to.
    Smart,
}

/// A Worker's resolved CPU limit and placement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerTuning {
    /// `None` leaves Cloudflare's default limit.
    pub cpu_ms: Option<u64>,
    pub placement: Placement,
}

/// A table's own keys, each `None` when it does not set it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerTuningOverride {
    pub cpu_ms: Option<u64>,
    pub placement: Option<Placement>,
}

impl WorkerTuningOverride {
    /// `base` with this table's keys laid over it.
    pub fn over(self, base: WorkerTuning) -> WorkerTuning {
        WorkerTuning {
            cpu_ms: self.cpu_ms.or(base.cpu_ms),
            placement: self.placement.unwrap_or(base.placement),
        }
    }

    /// Whether this table sets neither key.
    pub fn is_empty(self) -> bool {
        self.cpu_ms.is_none() && self.placement.is_none()
    }
}

/// Read the keys of `table` this module owns. The error is the reason, to
/// follow the table's name; a key it does not own is left to the caller.
pub fn from_table(table: &toml::Table) -> Result<WorkerTuningOverride, String> {
    let cpu_ms = match table.get("cpu_ms") {
        None => None,
        Some(toml::Value::Integer(n)) if (1..=MAX_CPU_MS as i64).contains(n) => Some(*n as u64),
        Some(toml::Value::Integer(n)) => {
            return Err(format!(
                "`cpu_ms` is {n}, but it must be from 1 to {MAX_CPU_MS} milliseconds (Cloudflare's maximum CPU limit)"
            ));
        }
        Some(_) => {
            return Err(
                "`cpu_ms` must be a whole number of milliseconds, like `cpu_ms = 50`".to_string(),
            );
        }
    };
    let placement = match table.get("placement") {
        None => None,
        Some(toml::Value::String(s)) if s == "smart" => Some(Placement::Smart),
        Some(toml::Value::String(s)) if s == "off" => Some(Placement::Off),
        Some(_) => {
            return Err("`placement` must be `\"smart\"` or `\"off\"`".to_string());
        }
    };
    Ok(WorkerTuningOverride { cpu_ms, placement })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(src: &str) -> Result<WorkerTuningOverride, String> {
        from_table(&src.parse::<toml::Table>().unwrap())
    }

    #[test]
    fn the_keys_read_and_an_override_lays_over_the_base() {
        let t = read("cpu_ms = 2000\nplacement = \"smart\"\n").unwrap();
        assert_eq!(t.cpu_ms, Some(2000));
        assert_eq!(t.placement, Some(Placement::Smart));
        let base = WorkerTuning {
            cpu_ms: Some(50),
            placement: Placement::Smart,
        };
        let group = read("placement = \"off\"\n").unwrap();
        assert_eq!(
            group.over(base),
            WorkerTuning {
                cpu_ms: Some(50),
                placement: Placement::Off
            }
        );
        assert!(read("").unwrap().is_empty());
    }

    #[test]
    fn out_of_range_and_wrongly_typed_values_are_refused() {
        assert!(
            read("cpu_ms = 0\n")
                .unwrap_err()
                .contains("from 1 to 300000")
        );
        assert!(read("cpu_ms = 300001\n").is_err());
        assert!(read("cpu_ms = 1.5\n").is_err());
        assert!(read("cpu_ms = \"50\"\n").is_err());
        assert!(read("placement = \"nearby\"\n").is_err());
        assert!(read("placement = true\n").is_err());
        assert!(read("cpu_ms = 300000\n").is_ok());
    }
}

//! The `@unique(by:)` duplicates a sweep finds already stored (#4390).
//!
//! A constraint added to a `Map` that already holds data is enforced only on new commits, so a value
//! two keys already hold stays until a commit touches it. Each woken instance reports, per
//! `@unique` index, how many holders each duplicated value has. This module reads that report,
//! gathers it for a whole class and words it.
//!
//! A report carries **no value-derived label**: the stored value, a hash of it and the keys holding it
//! are all left out. What is said is the map, the field, how many values are shared and by how many
//! holders each, which is enough to judge the clean-up without the data leaving the account.
//!
//! The finding is information, not a verdict: it never fails a sweep or holds a class's floor.

use std::collections::BTreeMap;

/// One `@unique` index of one instance that holds duplicates: `holders` has one number per value
/// that more than one key holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reported {
    pub map: String,
    pub by: String,
    pub holders: Vec<u64>,
}

/// Read the `unique` array of one woken instance's answer; absent or unreadable is none.
pub(crate) fn parse(woken: &serde_json::Value) -> Vec<Reported> {
    let Some(list) = woken.get("unique").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|r| {
            let text = |k: &str| {
                r.get(k)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            };
            let holders: Vec<u64> = r
                .get("holders")?
                .as_array()?
                .iter()
                .filter_map(serde_json::Value::as_u64)
                .filter(|n| *n > 1)
                .collect();
            if holders.is_empty() {
                return None;
            }
            Some(Reported {
                map: text("map")?,
                by: text("by")?,
                holders,
            })
        })
        .collect()
}

/// What a class's instances hold, per `(map, field)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Duplicates(BTreeMap<(String, String), Found>);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Found {
    /// The instances that hold at least one.
    instances: usize,
    /// The holders of every duplicated value, over all those instances.
    holders: Vec<u64>,
}

impl Duplicates {
    /// Take in one instance's report.
    pub fn absorb(&mut self, reported: Vec<Reported>) {
        for r in reported {
            let found = self.0.entry((r.map, r.by)).or_default();
            found.instances += 1;
            found.holders.extend(r.holders);
        }
    }

    /// One line per index, each ready to follow its class's summary: `2, 2 and 4 holders`.
    pub fn lines(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|((map, by), found)| {
                let mut holders = found.holders.clone();
                holders.sort_unstable();
                let values = holders.len();
                let noun = if values == 1 { "value is" } else { "values are" };
                format!(
                    "    @unique(by: {by}) on `{map}`: {values} {noun} held by more than one key: {} \
                     (in {} instance{}); the constraint refuses new duplicates only, so these stay until a \
                     commit changes them",
                    holders_phrase(&holders),
                    found.instances,
                    if found.instances == 1 { "" } else { "s" },
                )
            })
            .collect()
    }
}

/// `2 holders`, `2 and 4 holders`, `2, 2 and 4 holders`.
fn holders_phrase(holders: &[u64]) -> String {
    let numbers: Vec<String> = holders.iter().map(u64::to_string).collect();
    let list = match numbers.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    };
    format!("{list} holders")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reported(map: &str, by: &str, holders: &[u64]) -> Reported {
        Reported {
            map: map.into(),
            by: by.into(),
            holders: holders.to_vec(),
        }
    }

    #[test]
    fn the_holders_read_as_a_list() {
        assert_eq!(holders_phrase(&[3]), "3 holders");
        assert_eq!(holders_phrase(&[2, 4]), "2 and 4 holders");
        assert_eq!(holders_phrase(&[2, 2, 4]), "2, 2 and 4 holders");
    }

    #[test]
    fn an_instances_report_is_read_and_a_junk_one_is_none() {
        let woken = serde_json::json!({
            "unique": [
                { "map": "people", "by": "email", "holders": [4, 2, 2] },
                { "map": "x", "by": "y", "holders": [] },
                { "by": "no-map", "holders": [2] },
            ]
        });
        assert_eq!(parse(&woken), vec![reported("people", "email", &[4, 2, 2])]);
        assert!(parse(&serde_json::json!({})).is_empty());
        assert!(parse(&serde_json::json!({ "unique": "x" })).is_empty());
    }

    #[test]
    fn a_class_gathers_its_instances_per_index_and_names_no_value() {
        let mut d = Duplicates::default();
        assert!(d.lines().is_empty());
        d.absorb(vec![reported("people", "email", &[2, 4])]);
        d.absorb(vec![
            reported("people", "email", &[2]),
            reported("seats", "code", &[3]),
        ]);
        let lines = d.lines();
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].starts_with(
                "    @unique(by: email) on `people`: 3 values are held by more than one key: \
                 2, 2 and 4 holders (in 2 instances)"
            ),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].starts_with(
                "    @unique(by: code) on `seats`: 1 value is held by more than one key: 3 holders \
                 (in 1 instance)"
            ),
            "{}",
            lines[1]
        );
    }
}

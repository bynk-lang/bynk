//! `bynk test --case` / `--suite` (#2273): the set of tests one run executes.
//!
//! Both flags repeat. Values of one flag combine as **or**, the two flags as
//! **and**: a case runs when its suite is among the `--suite` values (when any
//! are given) and its name is among the `--case` values (when any are given).
//! The set reaches the emitted test entry as one JSON value in
//! `BYNK_TEST_FILTER`, so one process runs the whole selection, with one seed
//! and one coverage report.

use std::process::Command as ProcCommand;

use serde_json::json;

use crate::test_json::Unmatched;

/// The environment variable carrying the filter to the emitted test entry.
pub const FILTER_ENV: &str = "BYNK_TEST_FILTER";

/// Set by `bynk test --against dev` on each group's child run: the child sees
/// only its group's cases, so a value it matches nothing for is not an error
/// there. The parent judges every value against all the groups before it
/// starts one.
pub const PARTIAL_ENV: &str = "BYNK_TEST_PARTIAL_SELECTION";

/// The `--suite` and `--case` values of one run. Empty values are dropped and
/// duplicates removed, keeping the first occurrence, so an empty flag is unset
/// as `--case ""` always was.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestFilter {
    pub suites: Vec<String>,
    pub cases: Vec<String>,
    /// The run sees part of the selection (see [`PARTIAL_ENV`]), so a value
    /// that selects nothing is left unreported.
    pub partial: bool,
}

fn normalised(values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if !value.is_empty() && !out.contains(value) {
            out.push(value.clone());
        }
    }
    out
}

impl TestFilter {
    /// The filter for the `--suite` and `--case` values as given.
    pub fn new(suites: &[String], cases: &[String]) -> Self {
        Self {
            suites: normalised(suites),
            cases: normalised(cases),
            partial: false,
        }
    }

    /// This run is one group's part of a larger selection (see [`PARTIAL_ENV`]).
    pub fn from_environment(mut self) -> Self {
        self.partial = std::env::var_os(PARTIAL_ENV).is_some();
        self
    }

    /// No value is left, so every test runs.
    pub fn is_empty(&self) -> bool {
        self.suites.is_empty() && self.cases.is_empty()
    }

    /// The values that select nothing among `available` (suite, case) pairs: a
    /// suite no pair has, and a case no pair in a selected suite has. The
    /// runner judges the same way; `--against dev` asks it before starting
    /// anything, because it runs the selection in several processes.
    pub fn unmatched_in(&self, available: &[(String, String)]) -> Unmatched {
        let selected =
            |suite: &str| self.suites.is_empty() || self.suites.iter().any(|s| s == suite);
        Unmatched {
            suites: self
                .suites
                .iter()
                .filter(|s| !available.iter().any(|(suite, _)| suite == *s))
                .cloned()
                .collect(),
            cases: self
                .cases
                .iter()
                .filter(|c| {
                    !available
                        .iter()
                        .any(|(suite, case)| selected(suite) && case == *c)
                })
                .cloned()
                .collect(),
        }
    }

    /// The filter as the arguments that select it again, for a child `bynkc test`.
    pub fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        for suite in &self.suites {
            args.push("--suite".to_string());
            args.push(suite.clone());
        }
        for case in &self.cases {
            args.push("--case".to_string());
            args.push(case.clone());
        }
        args
    }

    /// Hand the filter to the runner `cmd` launches. `program` (`bynkc` or
    /// `bynk`) is carried so a rich run, whose output the driver cannot read,
    /// names the same program in its error as a JSON run does.
    pub fn apply(&self, program: &str, cmd: &mut ProcCommand) {
        if self.is_empty() {
            return;
        }
        let mut value = serde_json::Map::new();
        value.insert("program".to_string(), json!(program));
        if !self.suites.is_empty() {
            value.insert("suites".to_string(), json!(self.suites));
        }
        if !self.cases.is_empty() {
            value.insert("cases".to_string(), json!(self.cases));
        }
        if self.partial {
            value.insert("partial".to_string(), json!(true));
        }
        cmd.env(FILTER_ENV, serde_json::Value::Object(value).to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn drops_empty_values_and_duplicates_keeping_order() {
        let filter = TestFilter::new(&strings(&["b", "", "a", "b"]), &strings(&["x", "x", ""]));
        assert_eq!(filter.suites, strings(&["b", "a"]));
        assert_eq!(filter.cases, strings(&["x"]));
    }

    #[test]
    fn an_empty_flag_is_unset() {
        assert!(TestFilter::new(&strings(&[""]), &strings(&[""])).is_empty());
        assert!(TestFilter::new(&[], &[]).is_empty());
        assert!(!TestFilter::new(&[], &strings(&["x"])).is_empty());
    }

    #[test]
    fn a_name_with_a_comma_is_one_value() {
        let filter = TestFilter::new(&[], &strings(&["joins, groups, and orphans"]));
        assert_eq!(filter.cases, strings(&["joins, groups, and orphans"]));
    }

    #[test]
    fn judges_each_value_against_the_available_cases() {
        let available = vec![
            ("orders".to_string(), "places".to_string()),
            ("billing".to_string(), "charges".to_string()),
        ];
        let judge = |suites: &[&str], cases: &[&str]| {
            TestFilter::new(&strings(suites), &strings(cases)).unmatched_in(&available)
        };
        assert!(judge(&[], &[]).is_empty());
        assert!(judge(&["orders"], &["places"]).is_empty());
        assert_eq!(judge(&[], &["places", "typo"]).cases, strings(&["typo"]));
        assert_eq!(judge(&["nope"], &[]).suites, strings(&["nope"]));
        // A case that exists only outside the selected suites selects nothing.
        assert_eq!(
            judge(&["orders"], &["charges"]).cases,
            strings(&["charges"])
        );
    }

    #[test]
    fn args_repeat_each_flag() {
        let filter = TestFilter::new(&strings(&["s"]), &strings(&["a", "b"]));
        assert_eq!(
            filter.args(),
            strings(&["--suite", "s", "--case", "a", "--case", "b"])
        );
    }

    #[test]
    fn sets_the_environment_only_when_a_filter_was_given() {
        let envs = |filter: &TestFilter| {
            let mut cmd = ProcCommand::new("node");
            filter.apply("bynkc", &mut cmd);
            cmd.get_envs()
                .find(|(k, _)| *k == FILTER_ENV)
                .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        };
        assert_eq!(envs(&TestFilter::default()), None);
        let value = envs(&TestFilter::new(&strings(&["s"]), &strings(&["a \"b\""]))).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&value).unwrap();
        assert_eq!(
            parsed,
            json!({"program": "bynkc", "suites": ["s"], "cases": ["a \"b\""]})
        );
    }
}

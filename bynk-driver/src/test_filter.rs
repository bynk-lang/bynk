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

/// The `--suite` and `--case` values of one run. Empty values are dropped and
/// duplicates removed, keeping the first occurrence, so an empty flag is unset
/// as `--case ""` always was.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestFilter {
    pub suites: Vec<String>,
    pub cases: Vec<String>,
    /// The run sees only part of the selection (`--partial-selection`, which an
    /// orchestrator that runs the selection as several processes passes): it
    /// judges nothing, and leaves the unmatched values to its caller.
    pub partial: bool,
    /// The (suite, case) pairs the build skipped that this filter selects
    /// (`--against … --skip-unsupported`). They are not in the build, so the
    /// runner is told they exist: a selected skipped case is not unmatched.
    pub skipped: Vec<(String, String)>,
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
            skipped: Vec::new(),
        }
    }

    /// Mark this run as seeing only part of a selection.
    pub fn partial(mut self, partial: bool) -> Self {
        self.partial = partial;
        self
    }

    /// Whether `case` of `suite` is selected: its suite is among the `--suite`
    /// values (if any) and its name among the `--case` values (if any).
    pub fn selects(&self, suite: &str, case: &str) -> bool {
        (self.suites.is_empty() || self.suites.iter().any(|s| s == suite))
            && (self.cases.is_empty() || self.cases.iter().any(|c| c == case))
    }

    /// The same filter, told which of the build's skipped (suite, case) pairs it
    /// selects.
    pub fn with_skipped<'a>(
        mut self,
        skipped: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        self.skipped = skipped
            .into_iter()
            .filter(|(suite, case)| self.selects(suite, case))
            .map(|(suite, case)| (suite.to_string(), case.to_string()))
            .collect();
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
        if !self.skipped.is_empty() {
            let skipped: Vec<_> = self
                .skipped
                .iter()
                .map(|(suite, case)| json!({"suite": suite, "case": case}))
                .collect();
            value.insert("skipped".to_string(), json!(skipped));
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
    fn selects_by_suite_and_case_together() {
        let filter = TestFilter::new(&strings(&["a"]), &strings(&["x"]));
        assert!(filter.selects("a", "x"));
        assert!(!filter.selects("b", "x"));
        assert!(!filter.selects("a", "y"));
        assert!(TestFilter::default().selects("anything", "at all"));
    }

    #[test]
    fn tells_the_runner_which_skipped_cases_it_selects() {
        let filter = TestFilter::new(&strings(&["a"]), &[]).with_skipped([("a", "x"), ("b", "y")]);
        assert_eq!(filter.skipped, vec![("a".to_string(), "x".to_string())]);
        let mut cmd = ProcCommand::new("node");
        filter.apply("bynkc", &mut cmd);
        let value = cmd
            .get_envs()
            .find(|(k, _)| *k == FILTER_ENV)
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&value).unwrap();
        assert_eq!(parsed["skipped"], json!([{"suite": "a", "case": "x"}]));
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

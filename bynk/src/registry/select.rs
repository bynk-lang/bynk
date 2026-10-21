//! Version selection (packaging slice 4a, #2274, decision C): the highest
//! version satisfying every range that reaches a package, one version per
//! package, Cargo-style semver ranges.

use semver::{Version, VersionReq};

use super::protocol::IndexLine;

/// Why no version could be chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoVersion {
    /// No version satisfies every range.
    Conflict,
    /// Some versions satisfy every range, but each needs a newer `bynk`.
    Toolchain(String),
}

/// The line to use for a package, from its (verified) `lines`, given every
/// `range` that reaches it. `locked` is the locked version, which wins while
/// it still satisfies every range — even when yanked. Otherwise the highest
/// non-yanked version satisfying every range whose `toolchain` this `bynk`
/// meets.
pub(crate) fn choose<'l>(
    lines: &'l [IndexLine],
    ranges: &[String],
    locked: Option<&str>,
    toolchain: &Version,
) -> Result<&'l IndexLine, NoVersion> {
    let reqs: Vec<VersionReq> = ranges
        .iter()
        .filter_map(|r| VersionReq::parse(r).ok())
        .collect();
    let fits = |l: &IndexLine| {
        Version::parse(&l.version).is_ok_and(|v| reqs.iter().all(|r| r.matches(&v)))
    };
    if let Some(locked) = locked
        && let Some(l) = lines.iter().find(|l| l.version == locked)
        && fits(l)
    {
        return Ok(l);
    }
    let mut candidates: Vec<(Version, &IndexLine)> = lines
        .iter()
        .filter(|l| !l.yanked && fits(l))
        .filter_map(|l| Version::parse(&l.version).ok().map(|v| (v, l)))
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    let mut too_new: Option<String> = None;
    for (_, l) in candidates {
        match l.toolchain.as_deref().map(Version::parse) {
            Some(Ok(min)) if min > *toolchain => {
                too_new.get_or_insert_with(|| min.to_string());
            }
            _ => return Ok(l),
        }
    }
    Err(too_new.map_or(NoVersion::Conflict, NoVersion::Toolchain))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(version: &str, yanked: bool, toolchain: Option<&str>) -> IndexLine {
        IndexLine {
            name: "acme/money".into(),
            version: version.into(),
            checksum: String::new(),
            deps: vec![],
            npm: vec![],
            toolchain: toolchain.map(str::to_owned),
            yanked,
            sig: String::new(),
        }
    }

    #[test]
    fn the_highest_version_every_range_admits() {
        let lines = [
            line("1.2.0", false, None),
            line("1.4.2", false, None),
            line("1.5.0", true, None),
            line("2.0.0", false, None),
            line("1.4.9", false, Some("99.0.0")),
        ];
        let tc = Version::parse("0.383.0").unwrap();
        let pick = |ranges: &[&str], locked| {
            choose(
                &lines,
                &ranges.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                locked,
                &tc,
            )
            .map(|l| l.version.clone())
        };
        assert_eq!(
            pick(&["^1"], None),
            Ok("1.4.2".into()),
            "yanked 1.5.0 and too-new 1.4.9 skipped"
        );
        assert_eq!(pick(&["^1", ">=1.3"], None), Ok("1.4.2".into()));
        assert_eq!(pick(&["^1", "^2"], None), Err(NoVersion::Conflict));
        assert_eq!(
            pick(&["^1"], Some("1.5.0")),
            Ok("1.5.0".into()),
            "a locked yanked version holds"
        );
        assert_eq!(
            pick(&["^1.3"], Some("1.2.0")),
            Ok("1.4.2".into()),
            "a lock outside the range moves"
        );
        assert_eq!(
            pick(&["=1.4.9"], None),
            Err(NoVersion::Toolchain("99.0.0".into()))
        );
    }
}

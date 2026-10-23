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
    /// #2330 (B): the locked version still fits, but the registry withdrew
    /// it; carries the version and the (unsanitised) reason.
    Withdrawn(String, String),
}

/// The line to use for a package, from its (verified) `lines`, given every
/// `range` that reaches it. `locked` is the locked version, which wins while
/// it still satisfies every range — even when yanked, but not when withdrawn
/// (#2330: its archive is gone, so the lock can't be kept and nothing else
/// may silently replace it). Otherwise the highest non-yanked, non-withdrawn
/// version satisfying every range whose `toolchain` this `bynk` meets.
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
        return match &l.withdrawn {
            Some(reason) => Err(NoVersion::Withdrawn(l.version.clone(), reason.clone())),
            None => Ok(l),
        };
    }
    let mut candidates: Vec<(Version, &IndexLine)> = lines
        .iter()
        .filter(|l| !l.yanked && l.withdrawn.is_none() && fits(l))
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
            withdrawn: None,
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

    #[test]
    fn a_withdrawn_version_is_never_chosen_not_even_locked() {
        let mut gone = line("1.4.2", true, None);
        gone.withdrawn = Some("a takedown".into());
        // A mirror may serve `withdrawn` without `yanked`; it still excludes.
        let mut unyanked = line("1.5.0", false, None);
        unyanked.withdrawn = Some("malware".into());
        let lines = [line("1.2.0", false, None), gone, unyanked];
        let tc = Version::parse("0.383.0").unwrap();
        let ranges = vec!["^1".to_string()];
        assert_eq!(
            choose(&lines, &ranges, None, &tc).map(|l| l.version.clone()),
            Ok("1.2.0".into())
        );
        assert_eq!(
            choose(&lines, &ranges, Some("1.4.2"), &tc).map(|l| l.version.clone()),
            Err(NoVersion::Withdrawn("1.4.2".into(), "a takedown".into()))
        );
        // A lock outside the ranges moves as before, past the withdrawn ones.
        assert_eq!(
            choose(&lines, &["<1.3".to_string()], Some("1.4.2"), &tc).map(|l| l.version.clone()),
            Ok("1.2.0".into())
        );
    }
}

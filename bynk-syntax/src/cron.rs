//! #3873: the `Cron` refinement rule — exactly Cloudflare's cron-trigger syntax.
//!
//! [`is_cron_expression`] is the one compile-time statement of the rule the
//! `Cron` predicate enforces; its TypeScript twin is `__bynkIsCron`
//! (`bynk-emit/runtime/src/cron.ts`). Where Cloudflare's documentation does not
//! settle a form, the rule refuses it: a refusal can be relaxed later, an
//! admission can't be withdrawn.
//!
//! Five fields separated by exactly one space (no leading or trailing space):
//! minute `0-59`, hour `0-23`, day of month `1-31` (also `L`, `W`), month `1-12`
//! or `JAN`-`DEC`, day of week `1-7` with `1` = Sunday, or `SUN`-`SAT` (also
//! `L`, `#`). Names are case-insensitive. No seconds, year, `?` or `@daily`
//! macros.

/// Why a string is not a valid cron schedule: the failing field and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronFault {
    /// The field's name (`minute`, `hour`, `day of month`, `month`, `day of
    /// week`), or `schedule` for a fault in the whole string.
    pub field: &'static str,
    /// A sentence fragment saying what is wrong, e.g. `61 is outside 0-59`.
    pub reason: String,
}

impl CronFault {
    fn new(field: &'static str, reason: impl Into<String>) -> Self {
        Self {
            field,
            reason: reason.into(),
        }
    }

    /// The fault as one message: `minute 61 is outside 0-59`.
    pub fn message(&self) -> String {
        if self.field == "schedule" {
            self.reason.clone()
        } else {
            format!("{}: {}", self.field, self.reason)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Minute,
    Hour,
    DayOfMonth,
    Month,
    DayOfWeek,
}

impl Kind {
    const ALL: [Kind; 5] = [
        Kind::Minute,
        Kind::Hour,
        Kind::DayOfMonth,
        Kind::Month,
        Kind::DayOfWeek,
    ];

    fn name(self) -> &'static str {
        match self {
            Kind::Minute => "minute",
            Kind::Hour => "hour",
            Kind::DayOfMonth => "day of month",
            Kind::Month => "month",
            Kind::DayOfWeek => "day of week",
        }
    }

    fn range(self) -> (u32, u32) {
        match self {
            Kind::Minute => (0, 59),
            Kind::Hour => (0, 23),
            Kind::DayOfMonth => (1, 31),
            Kind::Month => (1, 12),
            Kind::DayOfWeek => (1, 7),
        }
    }

    fn names(self) -> &'static [&'static str] {
        match self {
            Kind::Month => &[
                "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
            ],
            Kind::DayOfWeek => &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"],
            _ => &[],
        }
    }
}

/// Is `s` a schedule Cloudflare's cron triggers accept? `Ok(())` if so, else the
/// first failing field.
pub fn is_cron_expression(s: &str) -> Result<(), CronFault> {
    if s.is_empty() {
        return Err(CronFault::new("schedule", "a schedule must not be empty"));
    }
    if s.starts_with(' ') || s.ends_with(' ') || s.contains("  ") || s.contains(['\t', '\n', '\r'])
    {
        return Err(CronFault::new(
            "schedule",
            "fields must be separated by exactly one space, with no leading or trailing space",
        ));
    }
    if s.starts_with('@') {
        return Err(CronFault::new(
            "schedule",
            "macros such as `@daily` are not supported; write the five fields",
        ));
    }
    let fields: Vec<&str> = s.split(' ').collect();
    if fields.len() != 5 {
        return Err(CronFault::new(
            "schedule",
            format!(
                "a schedule has exactly five fields (minute hour day-of-month month day-of-week), got {}",
                fields.len()
            ),
        ));
    }
    for (kind, field) in Kind::ALL.iter().zip(&fields) {
        check_field(*kind, field)?;
    }
    Ok(())
}

fn check_field(kind: Kind, field: &str) -> Result<(), CronFault> {
    let name = kind.name();
    if field.is_empty() {
        return Err(CronFault::new(name, "is empty"));
    }
    for item in field.split(',') {
        check_item(kind, item)?;
    }
    Ok(())
}

/// One comma-separated item: `*`, `*/n`, a value, a range, or a ranged step.
fn check_item(kind: Kind, item: &str) -> Result<(), CronFault> {
    let name = kind.name();
    if item.is_empty() {
        return Err(CronFault::new(name, "has an empty list item"));
    }
    if item.contains('?') {
        return Err(CronFault::new(name, "`?` is not supported; use `*`"));
    }
    let (lo, hi) = kind.range();
    let (body, step) = match item.split_once('/') {
        Some((b, st)) => (b, Some(st)),
        None => (item, None),
    };
    if let Some(st) = step {
        let n: u32 = st
            .parse()
            .map_err(|_| CronFault::new(name, format!("step `{st}` is not a number")))?;
        if n < 1 {
            return Err(CronFault::new(name, "a step must be at least 1"));
        }
        if n > hi {
            return Err(CronFault::new(
                name,
                format!("step {n} is larger than the field's maximum {hi}"),
            ));
        }
    }
    if body == "*" {
        return Ok(());
    }
    if let Some((a, b)) = body.split_once('-') {
        let a = value(kind, a)?;
        let b = value(kind, b)?;
        if a > b {
            return Err(CronFault::new(
                name,
                format!("range runs high to low ({a}-{b}); a range must run low to high"),
            ));
        }
        return Ok(());
    }
    if step.is_some() {
        // `n/step` (a start with no end) is not documented; refuse.
        return Err(CronFault::new(
            name,
            "a step applies to `*` or a range (`*/5`, `0-30/5`)",
        ));
    }
    special_or_value(kind, body, lo, hi)
}

/// A single value, or the `L`/`W`/`#` forms the day fields allow.
fn special_or_value(kind: Kind, body: &str, lo: u32, hi: u32) -> Result<(), CronFault> {
    let name = kind.name();
    match kind {
        Kind::DayOfMonth => {
            if body == "L" || body == "LW" {
                return Ok(());
            }
            if let Some(n) = body.strip_suffix('W') {
                let v = number(kind, n)?;
                return in_range(kind, v);
            }
        }
        Kind::DayOfWeek => {
            if let Some((d, nth)) = body.split_once('#') {
                let v = value(kind, d)?;
                let _ = v;
                let nth: u32 = nth
                    .parse()
                    .map_err(|_| CronFault::new(name, format!("`#{nth}` is not a number")))?;
                if !(1..=5).contains(&nth) {
                    return Err(CronFault::new(
                        name,
                        format!("`#{nth}` is outside 1-5 (the nth weekday of the month)"),
                    ));
                }
                return Ok(());
            }
            if let Some(d) = strip_suffix_ci(body, 'L') {
                // `6L` / `friL`: the last such weekday of the month. A bare
                // `L` is not documented, so it is refused.
                if d.is_empty() {
                    return Err(CronFault::new(
                        name,
                        "`L` needs a weekday before it (`6L`, `FRIL`)",
                    ));
                }
                value(kind, d)?;
                return Ok(());
            }
        }
        _ => {}
    }
    let _ = (lo, hi);
    value(kind, body).map(|_| ())
}

fn strip_suffix_ci(s: &str, c: char) -> Option<&str> {
    let last = s.chars().last()?;
    if last.eq_ignore_ascii_case(&c) {
        Some(&s[..s.len() - last.len_utf8()])
    } else {
        None
    }
}

fn number(kind: Kind, text: &str) -> Result<u32, CronFault> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CronFault::new(
            kind.name(),
            format!("`{text}` is not a valid value"),
        ));
    }
    text.parse::<u32>()
        .map_err(|_| CronFault::new(kind.name(), format!("`{text}` is out of range")))
}

fn in_range(kind: Kind, v: u32) -> Result<(), CronFault> {
    let (lo, hi) = kind.range();
    if (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(CronFault::new(
            kind.name(),
            format!("{v} is outside {lo}-{hi}"),
        ))
    }
}

/// A number in range, or (month, day of week) a three-letter name in any case.
fn value(kind: Kind, text: &str) -> Result<u32, CronFault> {
    if let Some(i) = kind
        .names()
        .iter()
        .position(|n| n.eq_ignore_ascii_case(text))
    {
        let (lo, _) = kind.range();
        return Ok(lo + i as u32);
    }
    let v = number(kind, text)?;
    in_range(kind, v)?;
    Ok(v)
}

/// #3873 [DECISION E]: the field text of a *valid* schedule, split for the
/// warnings. `None` if the string is not a single-spaced five-field schedule.
fn fields_of(s: &str) -> Option<Vec<&str>> {
    let f: Vec<&str> = s.split(' ').collect();
    (f.len() == 5).then_some(f)
}

/// `bynk.cron.numeric_weekday`: the day-of-week field names a day by number.
/// Standard cron's `5` is Friday, but Cloudflare's is Thursday (`1` = Sunday).
pub fn numeric_weekday(s: &str) -> bool {
    let Some(f) = fields_of(s) else { return false };
    let dow = f[4];
    // A numeric day anywhere: a digit before a `#`/`L`/`-`/`,`/`/` is a day;
    // the step after a `/` is not.
    dow.split(',').any(|item| {
        let body = item.split('/').next().unwrap_or("");
        body.split('-').any(|p| {
            p.trim_end_matches(['L', 'l'])
                .split('#')
                .next()
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

/// `bynk.cron.uneven_step`: a `*/n` in minute or hour where `n` does not divide
/// the field's range, with a description of when it actually fires.
pub fn uneven_step(s: &str) -> Option<String> {
    let f = fields_of(s)?;
    for (field, kind, size) in [(f[0], Kind::Minute, 60u32), (f[1], Kind::Hour, 24u32)] {
        for item in field.split(',') {
            let Some(n) = item.strip_prefix("*/").and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            if n >= 1 && size % n != 0 {
                let fires: Vec<String> = (0..size)
                    .step_by(n as usize)
                    .map(|v| v.to_string())
                    .collect();
                return Some(format!(
                    "`*/{n}` in the {} field fires at {} and then restarts at 0 at the next {}, so the last gap is shorter than {n}",
                    kind.name(),
                    fires.join(", "),
                    if kind == Kind::Minute { "hour" } else { "day" },
                ));
            }
        }
    }
    None
}

/// #3873: a valid schedule's five fields, named, for hover: `(field, text)`
/// pairs in order, e.g. `[("minute", "0"), ("hour", "2"), …]`. `None` unless
/// the schedule is valid.
pub fn describe_fields(s: &str) -> Option<Vec<(&'static str, String)>> {
    is_cron_expression(s).ok()?;
    Some(
        Kind::ALL
            .iter()
            .zip(s.split(' '))
            .map(|(k, f)| (k.name(), f.to_string()))
            .collect(),
    )
}

/// #3874: what a cron handler's parameter receives. A handler may declare up to
/// two, told apart by type, in either order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CronParam {
    /// An `Instant`: the scheduled time (`event.scheduledTime`).
    Time,
    /// Exactly `CronSchedule`: the schedule expression that fired the run
    /// (`event.cron`).
    Schedule,
}

/// #3874 [DECISION B]: the kind of a cron handler parameter by its declared
/// type: exactly `Instant`, or exactly the prelude's `CronSchedule` (not
/// `String`, and not another `Cron` refinement). `None` for any other type.
pub fn cron_param(ty: &crate::ast::TypeRef) -> Option<CronParam> {
    use crate::ast::{BaseType, TypeRef};
    match ty {
        TypeRef::Base(BaseType::Instant, _) => Some(CronParam::Time),
        TypeRef::Named(id) if id.name == "CronSchedule" => Some(CronParam::Schedule),
        _ => None,
    }
}

/// #3872: the name of the error type `E` in a cron handler's declared
/// `Effect[Result[(), E]]`, for the failure record a run leaves. `Error` when the
/// return type is not in that shape (the checker reports that separately) or the
/// error is not a named or base type.
pub fn cron_error_type(ret: &crate::ast::TypeRef) -> String {
    use crate::ast::TypeRef;
    if let TypeRef::Effect(inner, _) = ret
        && let TypeRef::Result(_, err, _) = &**inner
    {
        if let TypeRef::Named(name) = &**err {
            return name.name.clone();
        }
        if let TypeRef::Base(base, _) = &**err {
            return base.name().to_string();
        }
    }
    "Error".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) {
        assert_eq!(is_cron_expression(s), Ok(()), "{s:?} should be admitted");
    }
    fn bad(s: &str, field: &str) {
        let e = is_cron_expression(s).expect_err(s);
        assert_eq!(e.field, field, "{s:?}: {e:?}");
    }

    #[test]
    fn admits_every_example_on_cloudflares_page() {
        for s in [
            "* * * * *",
            "0 18 * * 6L",
            "0 18 * * friL",
            "59 23 LW * *",
            "*/45 * * * *",
            "0 2 * * MON-FRI",
            "0 2 * * mon-fri",
            "0 0 1,15 JAN,jul *",
            "0 */6 * * *",
            "5-10/2 * * * *",
            "0 0 15W * *",
            "0 0 L * *",
            "0 0 * * 6#2",
        ] {
            ok(s);
        }
    }

    #[test]
    fn refuses_each_malformed_schedule_naming_the_field() {
        bad("61 * * * *", "minute");
        bad("* * * * 0", "day of week");
        bad("* * * * 8", "day of week");
        bad("5-1 * * * *", "minute");
        bad("*/0 * * * *", "minute");
        bad("? * * * *", "minute");
        bad("@daily", "schedule");
        bad("0 0 * * * *", "schedule");
        bad("* * * *", "schedule");
        bad("* 24 * * *", "hour");
        bad("* * 0 * *", "day of month");
        bad("* * 32 * *", "day of month");
        bad("* * * 13 *", "month");
        bad("* * * FOO *", "month");
        bad("* * * * L", "day of week");
        bad("* * * * 1#6", "day of week");
    }

    #[test]
    fn refuses_irregular_whitespace() {
        for s in ["0  2 * * *", "0\t2 * * *", "0 2 * * * ", " 0 2 * * *"] {
            bad(s, "schedule");
        }
    }

    #[test]
    fn the_warnings() {
        assert!(numeric_weekday("* * * * 5"));
        assert!(numeric_weekday("* * * * 1-5"));
        assert!(!numeric_weekday("* * * * FRI"));
        assert!(!numeric_weekday("* * * * *"));
        assert!(!numeric_weekday("* * * * */2"));
        assert!(uneven_step("*/45 * * * *").is_some());
        assert!(uneven_step("*/15 * * * *").is_none());
        assert!(uneven_step("0 */5 * * *").is_some());
        assert!(uneven_step("0 */6 * * *").is_none());
    }
}

/// #3873: the cross-language corpus. A deterministic generator over the tokens
/// that matter (field values at their edges, names, the special characters,
/// stray spacing) writes `tests/cron_corpus.tsv`: one `ok`/`bad` verdict from
/// [`is_cron_expression`] and the string, as JSON. `bynk-emit/runtime`'s
/// `time.test.ts` replays it against `__bynkIsCron`, so the Rust rule and its
/// TypeScript twin cannot drift (the property test ADR 0727 uses for `IsoDate`).
#[cfg(test)]
mod corpus {
    use super::*;

    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/cron_corpus.tsv");

    fn generate() -> String {
        let mut seed: u64 = 3873;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        let atoms = [
            "*", "*/5", "*/0", "*/45", "*/60", "0", "1", "5", "7", "8", "12", "13", "23", "24",
            "31", "32", "59", "60", "0-5", "5-1", "1-7", "MON", "mon", "FRI", "JAN", "dec", "FOO",
            "MON-FRI", "FRI-MON", "6L", "friL", "L", "LW", "15W", "0W", "6#2", "1#6", "?", "5/10",
            "1,2", "1,", ",1", "0-30/5", "1-7/2", "*/", "-", "1-", "",
        ];
        let per_field: [&[&str]; 5] = [
            &[
                "*", "*/5", "*/15", "*/45", "0", "30", "59", "0-30", "5,10", "0-30/5",
            ],
            &["*", "*/6", "*/5", "0", "12", "23", "9-17", "0,12"],
            &[
                "*", "1", "15", "31", "L", "LW", "15W", "1-15", "*/2", "1,15",
            ],
            &["*", "1", "12", "JAN", "dec", "Jun", "JAN-MAR", "*/3", "1,6"],
            &[
                "*", "1", "7", "MON", "fri", "MON-FRI", "6L", "friL", "2#3", "1-5", "SUN,SAT",
            ],
        ];
        let mut out = String::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut push = |s: String, out: &mut String| {
            if seen.insert(s.clone()) {
                let verdict = if is_cron_expression(&s).is_ok() {
                    "ok"
                } else {
                    "bad"
                };
                out.push_str(verdict);
                out.push('\t');
                out.push_str(&serde_json_string(&s));
                out.push('\n');
            }
        };
        for s in [
            "* * * * *",
            "0 18 * * 6L",
            "0 18 * * friL",
            "59 23 LW * *",
            "@daily",
            "0 0 * * * *",
            "* * * *",
            "",
            " ",
            "0  2 * * *",
            "0\t2 * * *",
            "0 2 * * * ",
            " 0 2 * * *",
        ] {
            push(s.to_string(), &mut out);
        }
        for _ in 0..6000 {
            let n = if next() % 20 == 0 { 4 + next() % 3 } else { 5 };
            // Mostly each field's own plausible atoms, so many strings are valid;
            // sometimes any atom, so the edges are probed too.
            let fields: Vec<&str> = (0..n)
                .map(|i| {
                    if next() % 6 == 0 || i >= 5 {
                        atoms[next() % atoms.len()]
                    } else {
                        let own = per_field[i];
                        own[next() % own.len()]
                    }
                })
                .collect();
            let sep = if next() % 40 == 0 { "  " } else { " " };
            push(fields.join(sep), &mut out);
        }
        out
    }

    fn serde_json_string(s: &str) -> String {
        let mut o = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => o.push_str("\\\""),
                '\\' => o.push_str("\\\\"),
                '\t' => o.push_str("\\t"),
                '\n' => o.push_str("\\n"),
                '\r' => o.push_str("\\r"),
                c => o.push(c),
            }
        }
        o.push('"');
        o
    }

    #[test]
    fn corpus_is_current() {
        let want = generate();
        if std::env::var("BYNK_BLESS").is_ok() {
            std::fs::write(PATH, &want).unwrap();
        }
        let have = std::fs::read_to_string(PATH).unwrap_or_default();
        assert!(
            have == want,
            "bynk-syntax/tests/cron_corpus.tsv is stale; regenerate with BYNK_BLESS=1 cargo test -p bynk-syntax cron"
        );
        assert!(want.contains("ok\t") && want.contains("bad\t"));
    }
}

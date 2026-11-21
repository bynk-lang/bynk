//! When the driver's human output is coloured, and the few styles it uses.
//!
//! One decision for the whole process, set once from the global `--color`
//! (alias `--colour`) flag: `auto` (the default), `always` or `never`. Under
//! `auto` a stream is coloured when it is a terminal, or when `FORCE_COLOR` or
//! `CLICOLOR_FORCE` asks for colour without one, and never when `NO_COLOR` is
//! set (`allowed` has the exact rules). `always` and `never` override the
//! terminal check and the environment alike.
//!
//! The palette has one meaning per colour: blue for information, green for
//! success, red for an error or a failure, amber for a warning, and bold for a
//! heading.
//!
//! Machine-readable output (`--format json`, `ndjson`, and `bynk check`'s and
//! `bynk doctor`'s `short`) is never coloured, whatever the choice: only the
//! human renderings ask this module.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicU8, Ordering};

/// `--color <WHEN>`: when to colour the driver's human output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ColourChoice {
    /// Colour a terminal, unless `NO_COLOR` is set; `FORCE_COLOR` or
    /// `CLICOLOR_FORCE` colours one that isn't.
    #[default]
    Auto,
    /// Always colour, even into a pipe or a file.
    Always,
    /// Never colour.
    Never,
}

impl ColourChoice {
    /// clap's own choice for the same setting, for its help and errors.
    pub fn clap(self) -> clap::ColorChoice {
        match self {
            ColourChoice::Auto => clap::ColorChoice::Auto,
            ColourChoice::Always => clap::ColorChoice::Always,
            ColourChoice::Never => clap::ColorChoice::Never,
        }
    }

    /// The choice named by `--color`/`--colour` in `args` (the process's
    /// arguments, program name first), if one is given before a `--`. For
    /// clap's own help and errors, which are rendered while the arguments are
    /// parsed, so before [`set_choice`] can run. An unknown value is left for
    /// clap to report.
    pub fn from_args<I, S>(args: I) -> Option<ColourChoice>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        use clap::ValueEnum;
        let mut found = None;
        let mut args = args.into_iter().skip(1);
        while let Some(arg) = args.next() {
            let Some(arg) = arg.as_ref().to_str() else {
                continue;
            };
            if arg == "--" {
                break;
            }
            let value = match arg.split_once('=') {
                Some(("--color" | "--colour", value)) => value.to_string(),
                None if arg == "--color" || arg == "--colour" => {
                    match args
                        .next()
                        .and_then(|v| v.as_ref().to_str().map(str::to_string))
                    {
                        Some(value) => value,
                        None => break,
                    }
                }
                _ => continue,
            };
            // The last one wins, as it does for clap.
            if let Ok(choice) = ColourChoice::from_str(&value, true) {
                found = Some(choice);
            }
        }
        found
    }
}

static CHOICE: AtomicU8 = AtomicU8::new(0);

/// Set this process's choice (from `--color`). Until it is called the choice
/// is [`ColourChoice::Auto`].
pub fn set_choice(choice: ColourChoice) {
    CHOICE.store(choice as u8, Ordering::Relaxed);
}

/// This process's choice.
pub fn choice() -> ColourChoice {
    match CHOICE.load(Ordering::Relaxed) {
        1 => ColourChoice::Always,
        2 => ColourChoice::Never,
        _ => ColourChoice::Auto,
    }
}

/// Whether human output on stdout is coloured.
pub fn stdout() -> bool {
    resolve(choice(), || std::io::stdout().is_terminal())
}

/// Whether human output on stderr is coloured.
pub fn stderr() -> bool {
    resolve(choice(), || std::io::stderr().is_terminal())
}

fn resolve(choice: ColourChoice, is_terminal: impl FnOnce() -> bool) -> bool {
    match choice {
        ColourChoice::Always => true,
        ColourChoice::Never => false,
        ColourChoice::Auto => allowed(is_terminal(), |name| std::env::var_os(name)),
    }
}

/// Make a child process (a `wrangler`) whose output reaches the user colour
/// as we do: under `never`, `NO_COLOR` set and `FORCE_COLOR` taken away;
/// under `always`, the reverse. Under `auto` the child decides for itself,
/// unless its output is `piped` through us, where it can no longer see our
/// terminal: then `FORCE_COLOR` when we would colour stderr.
pub fn apply_to_child(cmd: &mut std::process::Command, piped: bool) {
    match choice() {
        ColourChoice::Never => {
            cmd.env_remove("FORCE_COLOR").env("NO_COLOR", "1");
        }
        ColourChoice::Always => {
            cmd.env_remove("NO_COLOR").env("FORCE_COLOR", "1");
        }
        ColourChoice::Auto => {
            if piped && stderr() {
                cmd.env("FORCE_COLOR", "1");
            }
        }
    }
}

/// #1666: under `auto`, colour needs a terminal, and `NO_COLOR` set to
/// anything but the empty string (<https://no-color.org>) turns it off. Colour
/// piped into a file or a CI log is noise.
///
/// #1777: `FORCE_COLOR` (<https://force-color.org>) or `CLICOLOR_FORCE`
/// (<https://bixense.com/clicolors/>), set to anything but the empty string or
/// `0`, turns colour on without a terminal, for `less -R` or a CI log that
/// renders ANSI. `0` means "don't force", not "force off": `NO_COLOR` is the
/// off switch, and it wins over both. That is clap's order for `NO_COLOR` and
/// `CLICOLOR_FORCE`; clap doesn't read `FORCE_COLOR`, so only
/// `CLICOLOR_FORCE` also colours clap's own help and errors.
pub(crate) fn allowed(is_terminal: bool, var: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    let set = |name| var(name).is_some_and(|v| !v.is_empty());
    let forced = |name| var(name).is_some_and(|v| !v.is_empty() && v != "0");
    if set("NO_COLOR") {
        false
    } else {
        is_terminal || forced("FORCE_COLOR") || forced("CLICOLOR_FORCE")
    }
}

/// A style the driver paints text in: one meaning per colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Information: a `note:`, a mutant that didn't compile.
    Blue,
    /// Success: a pass, a killed mutant, a check that passed.
    Green,
    /// A warning: partial coverage, a mutant no suite reaches.
    Amber,
    /// An error or a failure: a failing case, a surviving mutant, a refusal.
    Red,
    /// A heading.
    Bold,
    /// Detail the eye can skip.
    Dim,
}

impl Style {
    fn codes(self) -> (&'static str, &'static str) {
        match self {
            Style::Blue => ("\x1b[34m", "\x1b[39m"),
            Style::Green => ("\x1b[32m", "\x1b[39m"),
            Style::Amber => ("\x1b[33m", "\x1b[39m"),
            Style::Red => ("\x1b[31m", "\x1b[39m"),
            Style::Bold => ("\x1b[1m", "\x1b[22m"),
            Style::Dim => ("\x1b[2m", "\x1b[22m"),
        }
    }
}

/// `text` in `style` when `on`, otherwise `text` unchanged.
pub fn paint(on: bool, style: Style, text: &str) -> String {
    if on {
        let (open, close) = style.codes();
        format!("{open}{text}{close}")
    } else {
        text.to_string()
    }
}

/// `bynk test`'s human output (the runner's, as it is forwarded), coloured a
/// line at a time: a passing case's `✓` green, a failing case red, a suite
/// heading bold, a detached effect's report yellow, and the closing tally's
/// counts green and, when any failed, red. Any other line passes through as
/// it is. A heading is a line the runner starts after a blank one (each
/// suite's results end with one, and `Running tests...` with two), so a
/// test's own `console.log("Response:")` mid-suite is not taken for one.
#[derive(Debug, Default)]
pub struct TestLines {
    /// The line before was blank (or this is the first line).
    after_blank: bool,
}

impl TestLines {
    /// `line` coloured, its own end (`\n`, `\r\n` or none) kept.
    pub fn paint(&mut self, line: &str) -> String {
        let (body, end) = split_end(line);
        let after_blank = std::mem::replace(&mut self.after_blank, body.is_empty());
        let styled = if let Some(name) = body.strip_prefix("  \u{2713} ") {
            format!("  {} {name}", paint(true, Style::Green, "\u{2713}"))
        } else if body.starts_with("  \u{2717} ") {
            paint(true, Style::Red, body)
        } else if body.starts_with("    ! ") {
            paint(true, Style::Amber, body)
        } else if let Some(tally) = tally(body) {
            tally
        } else if after_blank && !body.starts_with(' ') && body.ends_with(':') {
            paint(true, Style::Bold, body)
        } else {
            return line.to_string();
        };
        format!("{styled}{end}")
    }
}

/// The runner's closing `<n> passed, <m> failed.`, coloured.
fn tally(body: &str) -> Option<String> {
    let (passed, rest) = body.split_once(" passed, ")?;
    let failed = rest.strip_suffix(" failed.")?;
    let p: u64 = passed.parse().ok()?;
    let f: u64 = failed.parse().ok()?;
    let passed = format!("{p} passed");
    let failed = format!("{f} failed");
    Some(format!(
        "{}, {}.",
        if p > 0 && f == 0 {
            paint(true, Style::Green, &passed)
        } else {
            passed
        },
        if f > 0 {
            paint(true, Style::Red, &failed)
        } else {
            paint(true, Style::Dim, &failed)
        },
    ))
}

fn split_end(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (line, "")
    }
}

/// A message for stderr (`bynk: warning: …`), with its `error:`, `warning:`
/// or `note:` label red, amber or blue: the first such label on each line,
/// whether it opens the line (after any indent or `= `) or follows the
/// `bynk …:` or `<program> …:` that names who is speaking. Nothing else
/// changes, so a line with no label is returned as it is.
pub fn message(text: &str) -> String {
    text.split_inclusive('\n').map(message_line).collect()
}

const LABELS: [(&str, Style); 3] = [
    ("error:", Style::Red),
    ("warning:", Style::Amber),
    ("note:", Style::Blue),
];

fn message_line(line: &str) -> String {
    // Where a label may start: after the indent and any `= `, and after each
    // `<speaker>: ` that opens the line, such as `bynk dev: `.
    let mut at = line.len() - line.trim_start().len();
    if line[at..].starts_with("= ") {
        at += 2;
    }
    let mut starts = vec![at];
    while let Some(colon) = line[at..].find(": ") {
        if !is_speaker(&line[at..at + colon]) {
            break;
        }
        at += colon + 2;
        starts.push(at);
    }
    for start in starts {
        for (label, style) in LABELS {
            if let Some(end) = label_end(&line[start..], label) {
                let (label, end) = (&line[start..start + end], start + end);
                return format!(
                    "{}{}{}",
                    &line[..start],
                    paint(true, style, label),
                    &line[end..]
                );
            }
        }
    }
    line.to_string()
}

/// Where `label` (`error:`) ends at the start of `text`, also in its coded
/// form, `error[bynk.fetch.status]:`, as the registry and fetch errors have it.
fn label_end(text: &str, label: &str) -> Option<usize> {
    if text.starts_with(label) {
        return Some(label.len());
    }
    let word = label.trim_end_matches(':');
    let code = text.strip_prefix(word)?.strip_prefix('[')?;
    let close = code.find("]:")?;
    let plain = |c: char| c.is_ascii_alphanumeric() || "._-".contains(c);
    (close > 0 && code[..close].chars().all(plain)).then_some(word.len() + 1 + close + 2)
}

/// A few plain words naming who speaks (`bynk`, `bynk dev`, `bynk test
/// --coverage`, or a workspace member, `packages/ok`), and never a label
/// itself.
fn is_speaker(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 40
        && !LABELS.iter().any(|(l, _)| l.trim_end_matches(':') == text)
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " -_./".contains(c))
}

/// `text` for stderr: its labels coloured as [`message`] does when stderr is
/// coloured, and unchanged when it isn't.
pub fn stderr_message(text: &str) -> String {
    if stderr() {
        message(text)
    } else {
        text.to_string()
    }
}

/// Print `text` and a newline to stderr, as [`stderr_message`] has it. What
/// [`emsg!`](crate::emsg) expands to.
pub fn eprint_message(text: &str) {
    eprintln!("{}", stderr_message(text));
}

/// `eprint!("{text}")`, coloured as [`stderr_message`] has it.
pub fn eprint(text: &str) {
    eprint!("{}", stderr_message(text));
}

/// `eprintln!("{text}")`, coloured as [`stderr_message`] has it.
pub fn eprintln(text: &str) {
    eprint_message(text);
}

/// `eprintln!` for a message with an `error:`, `warning:` or `note:` label,
/// coloured as [`colour::message`](crate::colour::message) does when stderr is.
#[macro_export]
macro_rules! emsg {
    ($($arg:tt)*) => {
        $crate::colour::eprint_message(&format!($($arg)*))
    };
}

/// The style for a coverage percentage: green from 80%, yellow (amber) from
/// 50%, red below.
pub fn coverage_style(percent: u32) -> Style {
    match percent {
        80.. => Style::Green,
        50.. => Style::Amber,
        _ => Style::Red,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1666: colour needs a terminal, and `NO_COLOR` set to anything but
    /// the empty string turns it off. #1777: `FORCE_COLOR` or
    /// `CLICOLOR_FORCE`, set to anything but the empty string or `0`, turns
    /// it on without one; `NO_COLOR` wins over both.
    #[test]
    fn auto_needs_a_terminal_or_a_force_and_no_no_color() {
        use std::ffi::OsString;
        let allowed = |is_terminal, vars: &[(&str, &str)]| {
            allowed(is_terminal, |name| {
                vars.iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| OsString::from(v))
            })
        };
        assert!(allowed(true, &[]));
        assert!(allowed(true, &[("NO_COLOR", "")]));
        assert!(!allowed(true, &[("NO_COLOR", "1")]));
        assert!(!allowed(false, &[]));
        assert!(!allowed(false, &[("NO_COLOR", "")]));

        for force in ["FORCE_COLOR", "CLICOLOR_FORCE"] {
            assert!(allowed(false, &[(force, "1")]), "{force}=1");
            assert!(allowed(false, &[(force, "true")]), "{force}=true");
            assert!(!allowed(false, &[(force, "")]), "{force}= is unset");
            assert!(!allowed(false, &[(force, "0")]), "{force}=0 doesn't force");
            assert!(allowed(true, &[(force, "0")]), "{force}=0 doesn't forbid");
            assert!(allowed(false, &[(force, "1"), ("NO_COLOR", "")]));
            assert!(!allowed(false, &[(force, "1"), ("NO_COLOR", "1")]));
            assert!(!allowed(true, &[(force, "1"), ("NO_COLOR", "1")]));
        }
    }

    #[test]
    fn always_and_never_ignore_the_terminal() {
        assert!(resolve(ColourChoice::Always, || false));
        assert!(!resolve(ColourChoice::Never, || true));
    }

    #[test]
    fn the_flag_is_found_in_either_spelling_and_the_last_wins() {
        let find = |args: &[&str]| ColourChoice::from_args(args);
        assert_eq!(find(&["bynk", "test"]), None);
        assert_eq!(
            find(&["bynk", "--color", "never", "test"]),
            Some(ColourChoice::Never)
        );
        assert_eq!(
            find(&["bynk", "test", "--colour=always"]),
            Some(ColourChoice::Always)
        );
        assert_eq!(
            find(&["bynk", "--color=never", "test", "--colour", "auto"]),
            Some(ColourChoice::Auto)
        );
        // After `--` it belongs to wrangler; an unknown value is clap's to report.
        assert_eq!(find(&["bynk", "dev", "--", "--color=never"]), None);
        assert_eq!(find(&["bynk", "--color=sometimes"]), None);
        assert_eq!(find(&["bynk", "--color"]), None);
    }

    /// Each line as the runner would print it, after a blank one.
    fn test_line(line: &str) -> String {
        TestLines { after_blank: true }.paint(line)
    }

    #[test]
    fn a_heading_follows_a_blank_line() {
        let mut lines = TestLines::default();
        let out: Vec<String> = [
            "Running tests...\n",
            "\n",
            "orders:\n",
            "Response:\n",
            "  \u{2713} adds\n",
            "\n",
            "billing:\n",
        ]
        .iter()
        .map(|l| lines.paint(l))
        .collect();
        assert_eq!(out[2], "\x1b[1morders:\x1b[22m\n");
        assert_eq!(out[3], "Response:\n", "a test's own output is left alone");
        assert_eq!(out[6], "\x1b[1mbilling:\x1b[22m\n");
    }

    #[test]
    fn test_lines_are_coloured_by_outcome() {
        assert_eq!(
            test_line("  \u{2713} adds\n"),
            "  \x1b[32m\u{2713}\x1b[39m adds\n"
        );
        assert_eq!(
            test_line("  \u{2717} subtracts\n"),
            "\x1b[31m  \u{2717} subtracts\x1b[39m\n"
        );
        assert_eq!(test_line("orders:\n"), "\x1b[1morders:\x1b[22m\n");
        assert_eq!(
            test_line("3 passed, 0 failed.\n"),
            "\x1b[32m3 passed\x1b[39m, \x1b[2m0 failed\x1b[22m.\n"
        );
        assert_eq!(
            test_line("3 passed, 1 failed."),
            "3 passed, \x1b[31m1 failed\x1b[39m."
        );
        // Anything else passes through untouched.
        for line in [
            "Running tests...\n",
            "\n",
            "    expected 1, got 2\n",
            "a passed, b failed.\n",
        ] {
            assert_eq!(test_line(line), line);
        }
    }

    #[test]
    fn a_message_label_is_coloured_where_it_stands() {
        let red = |l: &str| format!("\x1b[31m{l}\x1b[39m");
        let amber = |l: &str| format!("\x1b[33m{l}\x1b[39m");
        let blue = |l: &str| format!("\x1b[34m{l}\x1b[39m");
        assert_eq!(
            message("bynk: warning: kept it"),
            format!("bynk: {} kept it", amber("warning:"))
        );
        assert_eq!(
            message("bynk test: error: no tests found in x"),
            format!("bynk test: {} no tests found in x", red("error:"))
        );
        assert_eq!(
            message("bynk dev: note: `a.sql` changed"),
            format!("bynk dev: {} `a.sql` changed", blue("note:"))
        );
        assert_eq!(
            message("first\n  = note: the token came from X\n  note: y\nwarning: z"),
            format!(
                "first\n  = {} the token came from X\n  {} y\n{} z",
                blue("note:"),
                blue("note:"),
                amber("warning:")
            )
        );
        // Only the first label, and only where a label can stand.
        assert_eq!(
            message("bynk: warning: x said error: y"),
            format!("bynk: {} x said error: y", amber("warning:"))
        );
        // A workspace member speaks after `bynk:`, as a path.
        assert_eq!(
            message("bynk: packages/ok: error: no member"),
            format!("bynk: packages/ok: {} no member", red("error:"))
        );
        // The coded form takes the colour too, code and all.
        assert_eq!(
            message("bynk: error[bynk.fetch.status]: 404 from x"),
            format!("bynk: {} 404 from x", red("error[bynk.fetch.status]:"))
        );
        for plain in [
            "bynk: error[]: empty code",
            "bynk: error[a b]: a space",
            "bynk: signed out of `acme`",
            "bynk: could not read `a: b`: error: no",
            "",
        ] {
            assert_eq!(message(plain), plain);
        }
    }

    #[test]
    fn coverage_is_red_amber_green() {
        assert_eq!(coverage_style(100), Style::Green);
        assert_eq!(coverage_style(80), Style::Green);
        assert_eq!(coverage_style(79), Style::Amber);
        assert_eq!(coverage_style(50), Style::Amber);
        assert_eq!(coverage_style(49), Style::Red);
        assert_eq!(coverage_style(0), Style::Red);
    }

    #[test]
    fn paint_off_is_plain() {
        assert_eq!(paint(false, Style::Red, "x"), "x");
        assert_eq!(paint(true, Style::Red, "x"), "\x1b[31mx\x1b[39m");
    }
}

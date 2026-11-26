//! #4502 and #4451: the editor's view of the typed Analytics operations and the `{dataset}`
//! placeholder.
//!
//! - **Hover on a written field** shows the column `writeValue` puts it in (`blob3`, `double1`,
//!   `index1`), from the checked units' mapping ([`bynk_project::schema_registry::AnalyticsEntry`]).
//! - **Completion in the first argument** of `AnalyticsSql.query` / `select` offers the project's
//!   `Analytics` instances (#4451).
//! - **Completion of a typed read's fields**: a field-name position of a record that is the type
//!   argument of `AnalyticsSql.select[R]` offers the fields of the record its instance is written
//!   with that `R` does not declare yet.
//! - **Hover on `{dataset}`** inside the SQL literal names the table it stands for (#4451), and
//!   [`dataset_token_spans`] gives the placeholders their own semantic token.
//!
//! The call sites are found in the source text, the way the other argument-position completions
//! are, so they work in a buffer that does not check yet.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use bynk_project::schema_registry::AnalyticsEntry;
use bynk_syntax::ast::{CommonsItem, SourceUnit};

use crate::completion::{Completion, CompletionKind, for_each_unit};

/// A call of `AnalyticsSql.query(…)` or `AnalyticsSql.select[T](…)` in a source.
struct AnalyticsCall {
    /// `"select"` or `"query"`.
    op: &'static str,
    /// The `T` of `select[T]`.
    type_arg: Option<String>,
    /// The byte range of each top-level argument, in order; the last runs to the end of the text
    /// when the call is still open.
    args: Vec<(usize, usize)>,
}

/// Whether `text` consumes `name` from `bynk.cloudflare` (`consumes bynk.cloudflare { Ai, … }`), so
/// a name that is the project's own capability, not the first-party one, is not taken for it.
pub(crate) fn consumes_cloudflare(text: &str, name: &str) -> bool {
    text.match_indices("consumes bynk.cloudflare")
        .any(|(at, _)| {
            let rest = text[at + "consumes bynk.cloudflare".len()..].trim_start();
            let Some(inner) = rest.strip_prefix('{') else {
                return false;
            };
            let inner = inner.split('}').next().unwrap_or("");
            inner
                .split(|c: char| c == ',' || c.is_whitespace())
                .any(|word| word == name)
        })
}

/// The byte just past a string literal that starts at `from` (a `"` or a `"""`), or the end.
fn skip_string(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    if text[from..].starts_with("\"\"\"") {
        return text[from + 3..]
            .find("\"\"\"")
            .map_or(text.len(), |p| from + 3 + p + 3);
    }
    let mut i = from + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    text.len()
}

/// The top-level argument ranges of the call whose `(` is at `open`. A call that is never closed
/// (still being typed, or named in a comment) is read to the end of its own line only, so it
/// cannot claim the rest of the file.
fn scan_args(text: &str, open: usize) -> Vec<(usize, usize)> {
    if let Some(args) = scan_to(text, open, text.len(), true) {
        return args;
    }
    let line_end = text[open..].find('\n').map_or(text.len(), |p| open + p);
    scan_to(text, open, line_end, false).unwrap_or_default()
}

/// The argument ranges of the call opened at `open`, scanning no further than `limit`. `None` when
/// the call is not closed before `limit` and `need_close` holds.
fn scan_to(text: &str, open: usize, limit: usize, need_close: bool) -> Option<Vec<(usize, usize)>> {
    let bytes = &text.as_bytes()[..limit];
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut start = open + 1;
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i = skip_string(text, i);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    args.push((start, i));
                    return Some(args);
                }
                depth -= 1;
            }
            b',' if depth == 0 => {
                args.push((start, i));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if need_close {
        return None;
    }
    args.push((start, limit.max(start)));
    Some(args)
}

/// Every call of the two typed Analytics SQL operations in `text`.
fn analytics_calls(text: &str) -> Vec<AnalyticsCall> {
    let mut out = Vec::new();
    for (at, _) in text.match_indices("AnalyticsSql.") {
        // A mention in a line comment is not a call.
        let line_start = text[..at].rfind('\n').map_or(0, |p| p + 1);
        if text[line_start..at].contains("//") {
            continue;
        }
        let rest = &text[at + "AnalyticsSql.".len()..];
        let (op, after_op): (&'static str, usize) = if rest.starts_with("query") {
            ("query", "query".len())
        } else if rest.starts_with("select") {
            ("select", "select".len())
        } else {
            continue;
        };
        let mut i = at + "AnalyticsSql.".len() + after_op;
        let mut type_arg = None;
        if text[i..].starts_with('[') {
            let mut depth = 0usize;
            let mut close = None;
            for (k, c) in text[i..].char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(i + k);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(close) = close else { continue };
            type_arg = Some(text[i + 1..close].trim().to_string());
            i = close + 1;
        }
        if !text[i..].starts_with('(') {
            continue;
        }
        out.push(AnalyticsCall {
            op,
            type_arg,
            args: scan_args(text, i),
        });
    }
    out
}

/// The `Analytics` instances the project declares (`capability Signups = Analytics`), by name.
fn analytics_instances(text: &str, files: Option<&HashMap<PathBuf, String>>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for_each_unit(text, files, |unit| {
        let items = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            _ => return,
        };
        for item in items {
            if let CommonsItem::Capability(c) = item
                && c.derived
                    .as_ref()
                    .is_some_and(|d| d.base.name == "Analytics")
                && !found.contains(&c.name.name)
            {
                found.push(c.name.name.clone());
            }
        }
    });
    found.sort();
    found
}

/// #4451: the `Analytics` instances to offer when the cursor at `offset` is in the first argument of
/// `AnalyticsSql.query` or `AnalyticsSql.select`, or `None`.
pub fn instance_candidates_at(
    text: &str,
    offset: usize,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<Vec<Completion>> {
    let in_first_argument = analytics_calls(text).iter().any(|c| {
        c.args
            .first()
            .is_some_and(|&(s, e)| offset >= s && offset <= e)
    });
    if !in_first_argument {
        return None;
    }
    let names = analytics_instances(text, files);
    if names.is_empty() {
        return None;
    }
    Some(
        names
            .into_iter()
            .map(|n| {
                Completion::item(
                    n,
                    CompletionKind::Capability,
                    Some("the Analytics instance whose dataset `{dataset}` names".to_string()),
                )
            })
            .collect(),
    )
}

/// The column a mapping gives `field`, as `(column, kind)`.
fn column_of(entry: &AnalyticsEntry, field: &str) -> Option<(String, &'static str)> {
    if entry.index.as_deref() == Some(field) {
        return Some(("index1".to_string(), "sampling key"));
    }
    let find = |columns: &[String], family: &str| {
        columns
            .iter()
            .position(|c| c.split(':').next() == Some(field))
            .map(|i| {
                let kind = columns[i].rsplit(':').next().unwrap_or("");
                (format!("{family}{}", i + 1), kind_label(kind))
            })
    };
    find(&entry.blobs, "blob").or_else(|| find(&entry.doubles, "double"))
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "text" => "text",
        "enum" => "variant name",
        "bool" => "`\"true\"` / `\"false\"`",
        "number" => "number",
        "decimal" => "float value",
        _ => "value",
    }
}

/// #4502: the section to append to the hover of the record field `type_name.field`: the column an
/// `Analytics` instance written with that record puts it in. `None` when no instance is written
/// with the record or the field has no column (it is not written).
pub fn column_section(
    type_name: &str,
    field: &str,
    analytics: &BTreeMap<String, AnalyticsEntry>,
) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for (key, entry) in analytics {
        if entry.record != type_name {
            continue;
        }
        let Some((column, kind)) = column_of(entry, field) else {
            continue;
        };
        let instance = key.rsplit('.').next().unwrap_or(key);
        lines.push(format!("`{instance}` writes it to `{column}` ({kind})"));
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "\n\n**Analytics column:** {}. A change that moves it is refused by the schema lock (`bynk.analytics.mapping_changed`).",
        lines.join("; ")
    ))
}

/// The offset just after the `{` that opens the innermost record body the cursor is in, and the
/// name of the type it declares (`type R = {`), or `None` when the cursor is not at a field-name
/// position of a record type declaration.
fn record_being_declared(text: &str, offset: usize) -> Option<(String, usize)> {
    let before = text.get(..offset)?;
    let open = crate::completion::innermost_open_brace(text, offset)?;
    let head = before[..open].trim_end().trim_end_matches('=').trim_end();
    let name = head
        .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()?
        .to_string();
    let decl = head[..head.len() - name.len()].trim_end();
    if !decl.ends_with("type") {
        return None;
    }
    // A field-name position: nothing but a name begun since the last `,`, `{` or newline.
    let line = before[open + 1..].rsplit([',', '\n']).next().unwrap_or("");
    if line.contains(':') {
        return None;
    }
    Some((name, open))
}

/// #4502: the fields to offer at a field-name position of a record that is the type argument of an
/// `AnalyticsSql.select[R]`: the fields of the record the select's instance is written with, less
/// the ones `R` declares. `None` when the cursor is not in such a record, or the instance has no
/// recorded mapping.
pub fn read_field_candidates_at(
    text: &str,
    offset: usize,
    files: Option<&HashMap<PathBuf, String>>,
    analytics: &BTreeMap<String, AnalyticsEntry>,
) -> Option<Vec<Completion>> {
    let (record, open) = record_being_declared(text, offset)?;
    // The instance of the select that reads `record`, in this buffer or the project's files.
    let mut instance: Option<(String, Option<String>)> = None;
    let mut sources: Vec<&str> = vec![text];
    if let Some(files) = files {
        sources.extend(files.values().map(String::as_str));
    }
    for source in sources {
        for call in analytics_calls(source) {
            if call.op == "select"
                && call.type_arg.as_deref() == Some(record.as_str())
                && let Some(&(s, e)) = call.args.first()
            {
                instance = Some((source[s..e].trim().to_string(), context_name(source)));
                break;
            }
        }
        if instance.is_some() {
            break;
        }
    }
    let (instance, unit) = instance?;
    // The mapping of the select's own unit; failing that, the first instance of that name.
    let entry = unit
        .and_then(|u| analytics.get(&format!("{u}.{instance}")))
        .or_else(|| {
            analytics
                .iter()
                .find(|(k, _)| k.rsplit('.').next() == Some(instance.as_str()))
                .map(|(_, e)| e)
        })?;
    // The fields `record` already declares, up to the closing brace.
    let close = text[open..].find('}').map_or(text.len(), |p| open + p);
    let declared: Vec<&str> = text[open + 1..close]
        .split(',')
        .filter_map(|f| f.split(':').next())
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .collect();
    let mut candidates: Vec<(String, String)> = Vec::new();
    if let Some(i) = &entry.index {
        candidates.push((i.clone(), "index1 (sampling key)".to_string()));
    }
    for (family, columns) in [("blob", &entry.blobs), ("double", &entry.doubles)] {
        for (i, c) in columns.iter().enumerate() {
            let mut parts = c.split(':');
            let name = parts.next().unwrap_or("").to_string();
            let kind = parts.next().unwrap_or("");
            candidates.push((name, format!("{family}{} ({})", i + 1, kind_label(kind))));
        }
    }
    let out: Vec<Completion> = candidates
        .into_iter()
        .filter(|(n, _)| !declared.contains(&n.as_str()))
        .map(|(n, d)| {
            Completion::item(
                n,
                CompletionKind::Field,
                Some(format!("written by `{instance}`: {d}")),
            )
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

/// The byte ranges of every `{dataset}` placeholder inside the SQL argument of an
/// `AnalyticsSql.query` / `select` call.
fn placeholder_ranges(text: &str) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for call in analytics_calls(text) {
        let sql_arg = 1;
        let Some(&(s, e)) = call.args.get(sql_arg) else {
            continue;
        };
        let instance_arg = call.args.first().copied();
        let instance_start = instance_arg.map_or(0, |(a, _)| a);
        for (at, _) in text[s..e].match_indices("{dataset}") {
            out.push((s + at, s + at + "{dataset}".len(), instance_start));
        }
    }
    out
}

/// #4451: the spans of each `{dataset}` placeholder in the SQL argument of an Analytics SQL call,
/// for a semantic token of their own.
pub fn dataset_token_spans(text: &str) -> Vec<(usize, usize)> {
    placeholder_ranges(text)
        .into_iter()
        .map(|(s, e, _)| (s, e))
        .collect()
}

/// The context a source declares (`context shop.cart`), for the dataset's derived name.
fn context_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("context "))
        .map(|rest| rest.trim().trim_end_matches('{').trim().to_string())
}

/// #4451: the hover for a `{dataset}` placeholder under the cursor at `offset`: the table it
/// stands for, from the instance named in the call's first argument.
pub fn dataset_hover_at(text: &str, offset: usize) -> Option<String> {
    let (_, _, instance_start) = placeholder_ranges(text)
        .into_iter()
        .find(|&(s, e, _)| offset >= s && offset <= e)?;
    let instance: String = text[instance_start..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    let table = match (instance.is_empty(), context_name(text)) {
        // The binding's derivation (`bynk-emit`'s `analytics_dataset_name`): the Worker, then the
        // capability in snake case, all lowercase with `_` for `-`. The Worker is taken to be
        // the context's own, as it is unless the project groups contexts into one.
        (false, Some(ctx)) => Some(
            format!(
                "{}_{}",
                ctx.replace(['.', '-'], "_"),
                bynk_check::worker_bindings::screaming_snake(&instance)
            )
            .to_ascii_lowercase(),
        ),
        _ => None,
    };
    Some(match table {
        Some(table) => format!(
            "```\n{{dataset}}\n```\nThe table of `{instance}` for the environment the Worker runs in: `events.analyticsEngine.{table}`, or `events.analyticsEngine.{table}_staging` under `--env staging`. It is filled in by the binding, so the SQL stays the literal written here."
        ),
        None => "```\n{dataset}\n```\nThe table of the `Analytics` instance passed first, for the environment the Worker runs in. It is filled in by the binding, so the SQL stays the literal written here.".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "context shop.cart\n\nconsumes bynk.cloudflare { Analytics, AnalyticsSql }\n\ncapability Signups = Analytics\n\ntype Signup = { @index plan: String, country: String, seats: Int }\n\ntype ByCountry = {\n  country: String,\n  \n}\n\nservice api from http {\n  on GET(\"/x\") () -> Effect[HttpResult[Int]] by Visitor given AnalyticsSql, Signups {\n    let r <- AnalyticsSql.select[ByCountry](Signups, \", count() AS n FROM {dataset} WHERE timestamp > NOW() - INTERVAL '1' DAY\", [])\n    Ok(1)\n  }\n}\n";

    fn mapping() -> BTreeMap<String, AnalyticsEntry> {
        let mut m = BTreeMap::new();
        m.insert(
            "shop.cart.Signups".to_string(),
            AnalyticsEntry {
                record: "Signup".to_string(),
                index: Some("plan".to_string()),
                blobs: vec!["country:text".to_string()],
                doubles: vec!["seats:number".to_string()],
            },
        );
        m
    }

    #[test]
    fn a_written_fields_hover_names_its_column() {
        let s = column_section("Signup", "country", &mapping()).expect("a column");
        assert!(s.contains("`Signups` writes it to `blob1` (text)"), "{s}");
        let s = column_section("Signup", "seats", &mapping()).expect("a column");
        assert!(s.contains("`double1` (number)"), "{s}");
        let s = column_section("Signup", "plan", &mapping()).expect("a column");
        assert!(s.contains("`index1` (sampling key)"), "{s}");
        assert!(column_section("Signup", "missing", &mapping()).is_none());
        assert!(column_section("Other", "country", &mapping()).is_none());
    }

    #[test]
    fn the_first_argument_offers_the_projects_analytics_instances() {
        let at = SRC.find("select[ByCountry](").unwrap() + "select[ByCountry](".len();
        let got = instance_candidates_at(SRC, at, None).expect("offers");
        assert!(got.iter().any(|c| c.label == "Signups"), "{}", got.len());
        // Inside the SQL, nothing is offered.
        let sql = SRC.find("count()").unwrap();
        assert!(instance_candidates_at(SRC, sql, None).is_none());
    }

    #[test]
    fn a_typed_reads_record_offers_the_written_fields_it_lacks() {
        let at = SRC.find("  country: String,\n  \n}").unwrap() + "  country: String,\n  ".len();
        let got = read_field_candidates_at(SRC, at, None, &mapping()).expect("offers");
        let labels: Vec<&str> = got.iter().map(|c| c.label.as_str()).collect();
        assert!(
            labels.contains(&"plan") && labels.contains(&"seats"),
            "{labels:?}"
        );
        assert!(!labels.contains(&"country"), "already declared: {labels:?}");
        // A record no select reads offers nothing.
        let other = SRC.replace("select[ByCountry]", "select[Elsewhere]");
        assert!(read_field_candidates_at(&other, at, None, &mapping()).is_none());
    }

    #[test]
    fn a_dataset_placeholder_has_a_hover_and_a_token() {
        let at = SRC.find("{dataset}").unwrap() + 2;
        let spans = dataset_token_spans(SRC);
        assert_eq!(spans.len(), 1);
        assert_eq!(&SRC[spans[0].0..spans[0].1], "{dataset}");
        let hover = dataset_hover_at(SRC, at).expect("a hover");
        assert!(
            hover.contains("events.analyticsEngine.shop_cart_signups"),
            "{hover}"
        );
        assert!(hover.contains("_staging"), "{hover}");
        // Outside the SQL of an Analytics call, no hover.
        assert!(dataset_hover_at(SRC, 0).is_none());
    }

    /// A multi-word instance is in snake case in the table name, as the binding derives it.
    #[test]
    fn a_multi_word_instance_names_its_table_in_snake_case() {
        let src = SRC.replace("Signups", "PageViews");
        let at = src.find("{dataset}").unwrap() + 2;
        let hover = dataset_hover_at(&src, at).expect("a hover");
        assert!(
            hover.contains("events.analyticsEngine.shop_cart_page_views"),
            "{hover}"
        );
    }

    /// Two units writing an instance of one name: the select's own unit's mapping is offered.
    #[test]
    fn a_selects_own_units_mapping_is_preferred() {
        let mut m = mapping();
        m.insert(
            "a.first.Signups".to_string(),
            AnalyticsEntry {
                record: "Other".to_string(),
                index: None,
                blobs: vec!["zzz:text".to_string()],
                doubles: vec![],
            },
        );
        let at = SRC.find("  country: String,\n  \n}").unwrap() + "  country: String,\n  ".len();
        let got = read_field_candidates_at(SRC, at, None, &m).expect("offers");
        let labels: Vec<&str> = got.iter().map(|c| c.label.as_str()).collect();
        assert!(
            labels.contains(&"seats") && !labels.contains(&"zzz"),
            "{labels:?}"
        );
    }

    /// A mention in a comment, or a call left open, does not claim the rest of the file.
    #[test]
    fn a_comment_or_an_open_call_stays_contained() {
        let src = "/// e.g. AnalyticsSql.query(\ncapability Signups = Analytics\nlet x = 1\n";
        assert!(analytics_calls(src).is_empty());
        let open = "AnalyticsSql.query(\nlet later = 1, 2\n";
        let calls = analytics_calls(open);
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].args[0].1 <= "AnalyticsSql.query(".len(),
            "{:?}",
            calls[0].args
        );
    }

    #[test]
    fn consuming_a_name_from_the_cloudflare_unit_is_read_from_the_header() {
        let src = "context a\n\nconsumes bynk.cloudflare {\n  Ai,\n  AnalyticsSql\n}\n";
        assert!(consumes_cloudflare(src, "Ai") && consumes_cloudflare(src, "AnalyticsSql"));
        assert!(!consumes_cloudflare(src, "Kv"));
        assert!(!consumes_cloudflare(
            "context a\n\ncapability Ai {\n}\n",
            "Ai"
        ));
    }

    #[test]
    fn a_string_holding_a_comma_or_paren_does_not_split_the_arguments() {
        let src = "AnalyticsSql.query(Signups, \"SELECT count(), (1) FROM {dataset}\", [])";
        let calls = analytics_calls(src);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].args.len(), 3);
    }
}

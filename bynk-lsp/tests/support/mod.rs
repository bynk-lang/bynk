//! v0.121 (ADR 0156/0157): shared helpers for the editor-currency guardrail
//! tests — pure functions only. Slice C gave `bynk-lsp` a `[lib]` target, so
//! the source modules are now reached via `use bynk_lsp::…` rather than the
//! `#[path]`-include workaround these tests used before.

#![allow(dead_code)]

/// Strip VS Code snippet tab-stop syntax to a compilable skeleton:
/// `${N:default}` → `default`; `${N|a,b,c|}` → `a` (the first choice); a bare
/// `${N}` / `$N` / `$0` → `()` if it is both the *last* thing before a
/// closing `}` and inside an *expression* block (a body following `->`,
/// which cannot parse empty) — the empty parameter list if it sits directly
/// inside `( … )` — or empty otherwise (a declaration-list body, a cursor
/// trailing the whole construct, or a placeholder with real content still to
/// follow in the same block, can all be legitimately empty).
pub fn strip_snippet_placeholders(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut boundary = 0usize;
    let mut expr_block: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let rest = &body[i..];
        if let Some(inner) = rest.strip_prefix("${")
            && let Some(end) = inner.find('}')
        {
            let spec = &inner[..end];
            let consumed = "${".len() + end + "}".len();
            if let Some(colon) = spec.find(':') {
                out.push_str(&spec[colon + 1..]);
            } else if let Some(bar) = spec.find('|') {
                // `N|choice1,choice2,…|` — the first comma-separated choice.
                let rest = &spec[bar + 1..];
                let close = rest.rfind('|').unwrap_or(rest.len());
                let first_choice = rest[..close].split(',').next().unwrap_or("");
                out.push_str(first_choice);
            } else {
                let after = &body[i + consumed..];
                out.push_str(&bare_placeholder_fill(&out, after, &expr_block));
            }
            i += consumed;
            continue;
        }
        if let Some(stripped) = rest.strip_prefix('$') {
            let digits: usize = stripped
                .chars()
                .take_while(char::is_ascii_digit)
                .map(char::len_utf8)
                .sum();
            if digits > 0 {
                let after = &rest[1 + digits..];
                out.push_str(&bare_placeholder_fill(&out, after, &expr_block));
                i += 1 + digits;
                continue;
            }
        }
        let ch = rest.chars().next().expect("non-empty by the while guard");
        match ch {
            '{' => {
                // Immediately-preceding text since the last brace event: `->`
                // there means this block's body is an expression, not a
                // declaration list, so it cannot parse empty.
                expr_block.push(out[boundary..].contains("->"));
                out.push(ch);
                boundary = out.len();
            }
            '}' => {
                expr_block.pop();
                out.push(ch);
                boundary = out.len();
            }
            _ => out.push(ch),
        }
        i += ch.len_utf8();
    }
    out
}

fn bare_placeholder_fill(out_so_far: &str, after: &str, expr_block: &[bool]) -> String {
    let prev = out_so_far.trim_end().chars().last();
    let next = after.trim_start().chars().next();
    if prev == Some('(') && next == Some(')') {
        String::new()
    } else if expr_block.last() == Some(&true) && next == Some('}') {
        "()".to_string()
    } else {
        String::new()
    }
}

/// How a scaffold's stripped body must be wrapped before it parses as a
/// standalone `.bynk` source: some scaffolds are complete units, some are a
/// single item nested inside a unit, and some are a handler nested inside a
/// `service`.
enum Wrap {
    /// A complete `SourceUnit` on its own (`context`/`commons`/`adapter`/`suite`).
    Unit,
    /// One declaration, nested in a minimal fragment-form `context` header.
    Item,
    /// A handler body, nested in a minimal `service` inside that context.
    Handler,
    /// A test clause (`property`/`case`), nested in a minimal `suite`.
    Suite,
    /// A case-body statement (`advance Clock by …`, #2137), nested in a
    /// minimal `case` of a minimal `suite`.
    Case,
    /// A unit-header clause (`uses`/`consumes`), placed directly after a
    /// `context` header where those clauses grammatically belong.
    Header,
    /// An expression (`match`), nested in a minimal function body — it has no
    /// standalone item form.
    Expr,
}

fn classify(name: &str) -> Wrap {
    match name {
        "context" | "commons" | "adapter" | "suite" | "suite as" => Wrap::Unit,
        "property" | "case" | "case as" | "stub" => Wrap::Suite,
        "advance Clock by" => Wrap::Case,
        "uses" | "consumes" => Wrap::Header,
        "match" => Wrap::Expr,
        // Any `on <kind>(…)` handler — `on call`, `on http`, `on cron`, … —
        // parses only inside a `service`/`agent` body.
        other if other.starts_with("on ") => Wrap::Handler,
        other if other.starts_with("service") && other != "service" => Wrap::Handler,
        _ => Wrap::Item,
    }
}

/// Wrap a stripped scaffold body in the minimal skeleton it needs to parse —
/// the "compilable skeleton" ADR 0157 calls for.
pub fn wrap_for_parse(name: &str, stripped_body: &str) -> String {
    match classify(name) {
        Wrap::Unit => stripped_body.to_string(),
        Wrap::Item => format!("context Scaffold\n\n{stripped_body}\n"),
        Wrap::Handler => format!("context Scaffold\n\nservice Scaffold {{\n{stripped_body}\n}}\n"),
        Wrap::Suite => format!("suite Scaffold {{\n{stripped_body}\n}}\n"),
        Wrap::Case => format!("suite Scaffold {{\ncase \"t\" {{\n{stripped_body}\n}}\n}}\n"),
        Wrap::Header => format!("context Scaffold\n{stripped_body}\n"),
        Wrap::Expr => {
            format!("context Scaffold\n\nfn scaffold() -> Unit {{\n{stripped_body}\n}}\n")
        }
    }
}

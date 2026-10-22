//! #523: the comment-loss guard. Trivia attaches at declaration/statement
//! granularity, so a comment inside an expression subtree can vanish from the
//! formatted output. `format_source` must refuse (a visible no-op with a
//! `bynk.fmt.comment_loss` diagnostic) rather than write output that lost
//! user text.

use bynk_fmt::{FormatOptions, format_source};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_warnings;

fn expect_refusal(name: &str, source: &str) {
    match format_source(source, &FormatOptions::default()) {
        Ok(out) => {
            // If the formatter has since learned to preserve this placement,
            // the guard must not have fired — but the comment must be there.
            assert!(
                out.contains("keep me"),
                "{name}: formatted without refusing, but the comment vanished:\n{out}"
            );
        }
        Err(e) => {
            assert_eq!(e.errors.len(), 1, "{name}: expected one diagnostic");
            assert_eq!(
                e.errors[0].category, "bynk.fmt.comment_loss",
                "{name}: wrong category: {}",
                e.errors[0].category
            );
        }
    }
}

#[test]
fn refuses_to_drop_a_comment_between_binop_operands() {
    expect_refusal(
        "mid-binop",
        "commons demo\n\nfn f(x: Int) -> Int {\n  x +\n  // keep me\n  1\n}\n",
    );
}

#[test]
fn refuses_to_drop_a_comment_inside_a_record_literal() {
    expect_refusal(
        "in-record",
        "commons demo\n\ntype P = { x: Int, y: Int }\n\nfn f() -> P {\n  P {\n    x: 1,\n    // keep me\n    y: 2,\n  }\n}\n",
    );
}

#[test]
fn refuses_to_drop_a_comment_inside_a_match() {
    expect_refusal(
        "in-match",
        "commons demo\n\nfn f(o: Option[Int]) -> Int {\n  match o {\n    // keep me\n    Some(n) => n,\n    None => 0,\n  }\n}\n",
    );
}

#[test]
fn refuses_to_drop_a_comment_inside_a_list_literal() {
    expect_refusal(
        "in-list",
        "commons demo\n\nfn f() -> List[Int] {\n  [\n    1,\n    // keep me\n    2,\n  ]\n}\n",
    );
}

#[test]
fn refuses_to_drop_a_comment_after_the_tail_expression() {
    expect_refusal(
        "after-tail",
        "commons demo\n\nfn f() -> Int {\n  let x = 1\n  x\n  // keep me\n}\n",
    );
}

/// Statement-level comments are the supported placement — the guard must not
/// fire on a file the formatter handles correctly, including when formatting
/// moves the comment (leading-trivia normalisation) without losing it.
#[test]
fn statement_level_comments_format_without_refusal() {
    let source = "commons demo\n\n// module comment\nfn f(x: Int) -> Int {\n  // leading comment\n  let y = x // trailing comment\n  y\n}\n";
    let out = format_source(source, &FormatOptions::default()).expect("must format");
    for needle in ["module comment", "leading comment", "trailing comment"] {
        assert!(out.contains(needle), "lost `{needle}`:\n{out}");
    }
    // And the guard's contract holds transitively: formatting the output
    // again is loss-free and stable.
    let again = format_source(&out, &FormatOptions::default()).expect("must reformat");
    assert_eq!(out, again, "formatting must be idempotent");
}

/// #1664, #1756: a `---` block separated from the next declaration by a blank
/// line, or with nothing after it, is an orphan. The formatter once deleted it,
/// then (#1664) refused the file; it now keeps the block where it was. The
/// output must reformat to itself and still hold the orphan: formatting must
/// not attach it to the declaration below.
fn expect_kept(name: &str, source: &str) {
    let opts = FormatOptions::default();
    let out = format_source(source, &opts)
        .unwrap_or_else(|e| panic!("{name}: refused: {}", e.errors[0].message));
    assert!(out.contains("keep me"), "{name}: lost the block:\n{out}");
    let again = format_source(&out, &opts).expect("reformats");
    assert_eq!(out, again, "{name}: not idempotent");
    assert_eq!(
        orphan_warnings(&out),
        orphan_warnings(source),
        "{name}: formatting changed which blocks are orphans:\n{out}"
    );
}

fn orphan_warnings(source: &str) -> usize {
    let tokens = tokenize(source).expect("tokenises");
    let (_, warnings) = parse_unit_with_warnings(&tokens, source).expect("parses");
    warnings
        .iter()
        .filter(|w| w.category == "bynk.parse.orphan_doc_block")
        .count()
}

#[test]
fn keeps_an_orphan_doc_block_before_a_declaration() {
    expect_kept(
        "orphan before decl",
        "commons d\n\n---\nkeep me\n---\n\nfn f() -> Int { 1 }\n",
    );
}

#[test]
fn keeps_an_orphan_doc_block_between_declarations() {
    expect_kept(
        "orphan between decls",
        "commons d\n\nfn g() -> Int { 2 }\n\n---\nkeep me\n---\n\nfn f() -> Int { 1 }\n",
    );
}

#[test]
fn keeps_an_orphan_doc_block_at_end_of_file() {
    expect_kept("orphan at eof", "commons d\n\n---\nkeep me\n---\n");
}

/// In a brace-form body the block keeps its place and takes the body's indent,
/// before a declaration and before the closing brace.
#[test]
fn keeps_an_orphan_doc_block_in_a_brace_body() {
    expect_kept(
        "orphan in a brace body",
        "commons d {\n  ---\n  keep me\n  ---\n\n  fn f() -> Int { 1 }\n\n  ---\n  keep me too\n  ---\n}\n",
    );
}

/// Line comments on either side of an orphan stay on their side of it.
#[test]
fn keeps_an_orphan_between_line_comments() {
    let source = "commons d\n\n// above\n---\nkeep me\n---\n\n// below\nfn f() -> Int { 1 }\n";
    expect_kept("orphan between comments", source);
    let out = format_source(source, &FormatOptions::default()).unwrap();
    let above = out.find("// above").unwrap();
    let block = out.find("keep me").unwrap();
    let below = out.find("// below").unwrap();
    assert!(above < block && block < below, "{out}");
}

/// A `--` line directly under a doc block's closing `---` was binned as the
/// block's trailing comment, which nothing collects, so the file was refused
/// (#1756, found by `doc_blocks_are_always_kept`). It is a leading comment of
/// the declaration below, and the formatter moves it above the attached doc.
#[test]
fn keeps_a_comment_directly_under_a_doc_block() {
    let source = "commons d\n\n---\nattached\n---\n// keep me\nfn f() -> Int { 1 }\n";
    let out = format_source(source, &FormatOptions::default()).expect("formats");
    assert_eq!(
        out,
        "commons d\n\n// keep me\n---\nattached\n---\nfn f() -> Int {\n\t1\n}\n"
    );
}

/// #1756 review: a service, capability or agent body had no slot for what
/// comes before its closing `}`, so a `--` comment there was deleted with no
/// refusal, and an orphaned block made `fmt` refuse. Both are kept now.
#[test]
fn keeps_comments_and_an_orphan_at_the_end_of_a_service() {
    expect_kept(
        "end of service",
        "context c\n\nservice s {\n  on call() -> Effect[()] { Effect.pure(()) }\n\n  // keep me\n  ---\n  keep me too\n  ---\n}\n",
    );
    let out = format_source(
        "context c\n\nservice s {\n  on call() -> Effect[()] { Effect.pure(()) }\n\n  // keep me\n}\n",
        &FormatOptions::default(),
    )
    .unwrap();
    assert!(out.contains("// keep me"), "{out}");
}

#[test]
fn keeps_comments_and_an_orphan_at_the_end_of_a_capability() {
    expect_kept(
        "end of capability",
        "context c\n\ncapability K {\n  fn op() -> Effect[Int]\n\n  ---\n  keep me\n  ---\n  // and me\n}\n",
    );
}

#[test]
fn keeps_comments_and_an_orphan_at_the_end_of_an_agent() {
    expect_kept(
        "end of agent",
        "context c\n\nagent A {\n  key id: String\n  store n: Cell[Int] = 0\n  on call peek() -> Effect[Int] { Effect.pure(n) }\n\n  ---\n  keep me\n  ---\n}\n",
    );
}

/// A doc block above a service policy documents nothing: it warns as an
/// orphan whether or not a blank line separates it, and is kept.
#[test]
fn keeps_an_orphan_before_a_service_policy() {
    expect_kept(
        "before a policy",
        "context api\n\nservice api from http {\n  ---\n  keep me\n  ---\n  cors {\n    origins: [\"https://a.example.com\"],\n  }\n\n  on GET(\"/ping\") () -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"pong\")\n  }\n}\n",
    );
}

/// A doc block above an adapter's `uses` was dropped with no warning; it is an
/// orphan like any other now.
#[test]
fn keeps_an_orphan_before_an_adapter_clause() {
    let source = "adapter a\n\n---\nkeep me\n---\nuses bynk\n\ncapability K {\n  fn op() -> Effect[Int]\n}\n";
    assert_eq!(orphan_warnings(source), 1);
    expect_kept("before adapter uses", source);
}

/// #1786: a comment inside a `cors`/`security`/`limits` policy had no slot. It
/// was deleted with no refusal, then (#1785) refused. It is kept now.
#[test]
fn keeps_a_comment_inside_a_service_policy() {
    expect_kept(
        "in-policy",
        "context api\n\nservice api from http {\n  cors {\n    // keep me\n    origins: [\"https://a.example.com\"],\n  }\n\n  on GET(\"/ping\") () -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"pong\")\n  }\n}\n",
    );
}

/// #1786: every place a comment can sit in a policy keeps it there: above a
/// field, at the end of a field's line, before `}`, after `}`, and an orphaned
/// doc block between fields, which also warns.
#[test]
fn keeps_comments_in_every_policy_position() {
    let source = "context api\n\nservice api from http {\n  cors {\n    // above\n    origins: [\"https://a.example.com\"], // same line\n    ---\n    keep me\n    ---\n\n    credentials: false\n    // before close\n  } // after close\n  security {\n    hsts: 180.days, // on security\n  }\n  limits {\n    // on limits\n    maxBody: 1_048_576,\n  }\n\n  on GET(\"/ping\") () -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"pong\")\n  }\n}\n";
    assert_eq!(orphan_warnings(source), 1);
    expect_kept("policy positions", source);
    let out = format_source(source, &FormatOptions::default()).unwrap();
    for (before, after) in [
        ("// above", "origins:"),
        ("origins:", "// same line"),
        ("// same line", "keep me"),
        ("keep me", "credentials:"),
        ("credentials:", "// before close"),
        ("// before close", "// after close"),
        ("hsts:", "// on security"),
        ("// on limits", "maxBody:"),
    ] {
        assert!(
            out.find(before).unwrap() < out.find(after).unwrap(),
            "`{before}` should precede `{after}`:\n{out}"
        );
    }
    // The whole line, so a comment hoisted onto a line of its own fails.
    for line in [
        "origins: [\"https://a.example.com\"],  // same line\n",
        "hsts: 180.days,  // on security\n",
    ] {
        assert!(
            out.contains(line),
            "`{line}` stays on its field's line:\n{out}"
        );
    }
}

/// #1786 review: an orphan as the last thing before a policy's `}` (it ends the
/// policy's `trailing_comments`, so no blank line follows it), and a policy
/// holding nothing but a comment.
#[test]
fn keeps_an_orphan_closing_a_policy_and_a_comment_only_policy() {
    let wrap = |cors: &str| {
        format!(
            "context api\n\nservice api from http {{\n  cors {{\n{cors}  }}\n\n  on GET(\"/ping\") () -> Effect[HttpResult[String]] by v: Visitor {{\n    Ok(\"pong\")\n  }}\n}}\n"
        )
    };
    let closing =
        wrap("    origins: [\"https://a.example.com\"],\n    ---\n    keep me\n    ---\n");
    assert_eq!(orphan_warnings(&closing), 1);
    expect_kept("orphan closing a policy", &closing);
    let out = format_source(&closing, &FormatOptions::default()).unwrap();
    assert!(
        out.contains("keep me\n\t\t---\n\t}"),
        "no blank line before `}}`:\n{out}"
    );
    expect_kept("comment-only policy", &wrap("    // keep me\n"));
}

/// The block before the unit header is kept too.
#[test]
fn keeps_an_orphan_doc_block_before_the_header() {
    expect_kept(
        "orphan before header",
        "---\nkeep me\n---\n\ncommons d\n\nfn f() -> Int { 1 }\n",
    );
}

/// The doc-block guard runs whether or not any `--` comment needed a slot, and
/// an attached block, re-indented inside a brace body, is not mistaken for a
/// lost one.
#[test]
fn an_attached_doc_block_formats() {
    let src = "context c {\n---\n  keep me\n---\nfn f() -> Int { 1 }\n}\n";
    let out = format_source(src, &FormatOptions::default()).expect("an attached doc formats");
    assert!(out.contains("keep me"), "{out}");
}

/// #1788: a comment on the same line as an opening `{` was filed as the `{`'s
/// trailing trivia, which no parser collected, so the file was refused. It now
/// leads whatever follows the brace: the first item, or the `}` of an empty
/// body. `fmt` moves it onto its own line under the brace.
#[test]
fn keeps_a_comment_on_the_line_of_an_opening_brace() {
    let policy = |name: &str, field: &str| {
        format!(
            "context api\n\nservice api from http {{\n  {name} {{ // keep me\n    {field}\n  }}\n\n  on GET(\"/ping\") () -> Effect[HttpResult[String]] by v: Visitor {{\n    Ok(\"pong\")\n  }}\n}}\n"
        )
    };
    for (name, source) in [
        (
            "commons",
            "commons d { // keep me\n  fn f() -> Int { 1 }\n}\n".to_string(),
        ),
        (
            "context",
            "context c { // keep me\n  fn f() -> Int { 1 }\n}\n".to_string(),
        ),
        (
            "suite",
            "suite demo.wallet { // keep me\n  property \"p\" {\n    for all n: Int { expect n == n }\n  }\n}\n"
                .to_string(),
        ),
        (
            "service",
            "context c\n\nservice s { // keep me\n  on call() -> Effect[()] { Effect.pure(()) }\n}\n"
                .to_string(),
        ),
        (
            "agent",
            "context c\n\nagent A { // keep me\n  key id: String\n  store n: Cell[Int]\n\n  on call get() -> Effect[Int] {\n    n\n  }\n}\n"
                .to_string(),
        ),
        (
            "capability",
            "context c\n\ncapability K { // keep me\n  fn op() -> Effect[Int]\n}\n".to_string(),
        ),
        ("cors", policy("cors", "origins: [\"https://a.example.com\"],")),
        ("security", policy("security", "hsts: 180.days,")),
        ("limits", policy("limits", "maxBody: 1_048_576,")),
        (
            "fn",
            "commons d\n\nfn f() -> Int { // keep me\n  let x = 1\n  x\n}\n".to_string(),
        ),
        (
            "fn tail",
            "commons d\n\nfn f() -> Int { // keep me\n  1\n}\n".to_string(),
        ),
        (
            "record",
            "commons d\n\ntype P = { // keep me\n  x: Int,\n}\n".to_string(),
        ),
        (
            "doc block after the brace",
            "context c\n\nservice s { // keep me\n  ---\n  doc\n  ---\n  on call() -> Effect[()] { Effect.pure(()) }\n}\n"
                .to_string(),
        ),
        (
            "provider",
            "context c\n\ncapability K {\n  fn op() -> Effect[Int]\n}\n\nprovides K = Fixed { // keep me\n  fn op() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n"
                .to_string(),
        ),
        (
            "adapter",
            "adapter a { // keep me\n  binding \"./a.binding.ts\"\n\n  capability K {\n    fn op() -> Effect[Int]\n  }\n}\n"
                .to_string(),
        ),
        (
            "event",
            "context c\n\nevent E = { // keep me\n  x: Int,\n}\n".to_string(),
        ),
        (
            "event with a comment after its brace",
            "context c\n\nevent E = { // keep me\n  x: Int,\n} // after close\n".to_string(),
        ),
        ("empty commons", "commons d { // keep me\n}\n".to_string()),
        (
            "empty record",
            "commons d\n\ntype P = { // keep me\n}\n".to_string(),
        ),
        (
            "empty policy",
            policy("cors", ""),
        ),
    ] {
        expect_kept(name, &source);
        let out = format_source(&source, &FormatOptions::default()).unwrap();
        let line = out.lines().find(|l| l.contains("keep me")).unwrap();
        assert_eq!(line.trim(), "// keep me", "{name}: moved onto its own line:\n{out}");
    }
}

/// #1788: the brace-line comment of a record type and of an agent had nowhere
/// to go, because a record field and an agent's `key` had no comment slot.
/// Now every position in either keeps its comment there.
#[test]
fn keeps_comments_in_a_record_type_and_on_an_agent_key() {
    let record = "commons d\n\ntype P = {\n  // above\n  x: Int, // same line\n  y: Int // last\n  // before close\n} // after close\n";
    let agent = "context c\n\nagent A {\n  // above key\n  key id: String // on key\n  store n: Cell[Int]\n\n  on call get() -> Effect[Int] {\n    n\n  }\n}\n";
    for (name, source, lines) in [
        (
            "record",
            record,
            &[
                "\t// above\n\tx: Int,  // same line\n",
                "\ty: Int,  // last\n\t// before close\n}  // after close\n",
            ][..],
        ),
        (
            "agent key",
            agent,
            &["\t// above key\n\tkey id: String  // on key\n"][..],
        ),
    ] {
        let out = format_source(source, &FormatOptions::default())
            .unwrap_or_else(|e| panic!("{name}: refused: {}", e.errors[0].message));
        for line in lines {
            assert!(out.contains(line), "{name}: `{line}` not in place:\n{out}");
        }
        let again = format_source(&out, &FormatOptions::default()).expect("reformats");
        assert_eq!(out, again, "{name}: not idempotent");
    }
}

/// #1794: a sum-type variant had no comment slot, so `fmt` refused a comment
/// anywhere inside an `enum { … }` or pipe-form sum. Each position keeps its
/// comment on its line now. A payloadless pipe form prints as an `enum`, so
/// the pipe rows give their variants a payload.
#[test]
fn keeps_comments_in_a_sum_type() {
    let pipe = "commons d\n\ntype S = // on eq\n  // above first\n  | A(x: Int) // same line\n  // above\n  | B // bare\n  | C(y: Int) // last\n";
    let embeds = "commons d\n\ntype S = | A(x: Int) // before embeds\n  // own line\n  embeds Foo as A // after embeds\n";
    for (name, source, lines) in [
        (
            "enum",
            "commons d\n\ntype E = enum { // brace line\n  // above\n  A, // same line\n  B // last\n  // before close\n} // after close\n",
            &[
                "enum {\n\t// brace line\n\t// above\n\tA,  // same line\n",
                "\tB,  // last\n\t// before close\n}  // after close\n",
            ][..],
        ),
        (
            "empty enum",
            "commons d\n\ntype E = enum { // keep me\n}\n",
            &["enum {\n\t// keep me\n}\n"][..],
        ),
        (
            "enum on eq",
            "commons d\n\ntype E = // on eq\n  enum { A, B }\n",
            &["enum {\n\t// on eq\n\tA,\n"][..],
        ),
        (
            "empty enum on eq",
            "commons d\n\ntype E = // on eq\n  enum {}\n",
            &["enum {\n\t// on eq\n}\n"][..],
        ),
        (
            "pipe",
            pipe,
            &[
                "type S =\n// on eq\n// above first\n| A(x: Int)  // same line\n",
                "// above\n| B  // bare\n| C(y: Int)  // last\n",
            ][..],
        ),
        (
            "pipe as enum",
            "commons d\n\ntype S = | A // same line\n  | B\n",
            &["enum {\n\tA,  // same line\n\tB,\n}\n"][..],
        ),
        (
            "pipe with embeds",
            embeds,
            &["| A(x: Int)  // before embeds\n// own line\nembeds Foo as A  // after embeds\n"][..],
        ),
    ] {
        let out = format_source(source, &FormatOptions::default())
            .unwrap_or_else(|e| panic!("{name}: refused: {}", e.errors[0].message));
        for line in lines {
            assert!(out.contains(line), "{name}: `{line}` not in place:\n{out}");
        }
        let again = format_source(&out, &FormatOptions::default()).expect("reformats");
        assert_eq!(out, again, "{name}: not idempotent");
    }
}

/// #1797: a comment inside an actor body or an `exports` list is kept: on the
/// `{` line, above an entry, at the end of its line, and before the `}`. The
/// OIDC actor keeps its arguments one per line (#963), so the comments ride
/// that form too, and the empty `exports` list stays open for its comment.
#[test]
fn keeps_comments_in_an_actor_body_and_an_exports_list() {
    let actor = "context c\n\nactor P { // the signed-in user\n  auth = Bearer(secret = \"S\"), // on auth\n  // above identity\n  identity = UserId // on identity\n  // before close\n} // after close\n\nactor Q {\n  auth = None // open\n}\n";
    let oidc = "context c\n\nactor O {\n  // the issuer\n  auth = Oidc(issuer = \"https://issuer.example.com/realms/main\", audience = \"https://api.example.com\"), // verified\n  identity = UserId\n}\n";
    let adapter = "adapter a {\n  binding \"./a.ts\"\n\n  exports capability { // offered\n    Cap // the one\n  }\n\n  capability Cap {\n    fn f() -> Effect[Int]\n  }\n}\n";
    let exports = "context c\n\nexports opaque { // the public ids\n  P, // the first\n  // above Q\n  Q\n  // before close\n} // after close\nexports capability {\n  // none yet\n}\n\ntype P = Int\n";
    for (name, source, lines) in [
        (
            "actor",
            actor,
            &[
                "actor P {\n\t// the signed-in user\n\tauth = Bearer(secret = \"S\"),  // on auth\n",
                "\t// above identity\n\tidentity = UserId  // on identity\n\t// before close\n}  // after close\n",
                "actor Q {\n\tauth = None  // open\n}\n",
            ][..],
        ),
        (
            "oidc actor",
            oidc,
            &[
                "actor O {\n\t// the issuer\n\tauth = Oidc(\n",
                "\t),  // verified\n\tidentity = UserId\n}\n",
            ][..],
        ),
        (
            "adapter exports",
            adapter,
            &["\texports capability {\n\t\t// offered\n\t\tCap,  // the one\n\t}\n"][..],
        ),
        (
            "exports",
            exports,
            &[
                "exports opaque {\n\t// the public ids\n\tP,  // the first\n",
                "\t// above Q\n\tQ,\n\t// before close\n}  // after close\n",
                "exports capability {\n\t// none yet\n}\n",
            ][..],
        ),
    ] {
        let out = format_source(source, &FormatOptions::default())
            .unwrap_or_else(|e| panic!("{name}: refused: {}", e.errors[0].message));
        for line in lines {
            assert!(out.contains(line), "{name}: `{line}` not in place:\n{out}");
        }
        let again = format_source(&out, &FormatOptions::default()).expect("reformats");
        assert_eq!(out, again, "{name}: not idempotent");
    }
}

/// #1797: without a trailing comma the last exported name drops its `,` but
/// keeps its end-of-line comment, and the actor's `identity` line is unchanged.
#[test]
fn keeps_an_end_of_line_comment_on_the_last_name_without_a_trailing_comma() {
    let source =
        "context c\n\nexports opaque {\n  P, // the first\n  Q // the last\n}\n\ntype P = Int\n";
    let opts = FormatOptions {
        trailing_comma: false,
        ..FormatOptions::default()
    };
    let out = format_source(source, &opts).expect("formats");
    assert!(
        out.contains("\tP,  // the first\n\tQ  // the last\n}\n"),
        "last name not in place:\n{out}"
    );
    assert_eq!(
        out,
        format_source(&out, &opts).expect("reformats"),
        "not idempotent"
    );
}

/// #1797's boundary: the scheme's arguments carry no trivia, so a comment
/// between two of them is still refused rather than dropped.
#[test]
fn refuses_to_drop_a_comment_between_actor_scheme_arguments() {
    expect_refusal(
        "in-scheme-args",
        "context c\n\nactor O {\n  auth = Oidc(issuer = \"a\", // keep me\n  audience = \"b\")\n}\n",
    );
}

/// #1808, #1859: a trailing comment ends its line and changes no vertical
/// spacing. A suite `uses` and an agent `store` field each added a second
/// newline after the comment: a blank line the uncommented form doesn't have.
/// A suite body separates every item with one blank line, `uses` included
/// (unlike a context's header `uses`), and #1808 keeps that: the defect was
/// the second one. A multi-line `store` initialiser takes its comment on
/// its closing line.
#[test]
fn a_trailing_comment_adds_no_blank_line() {
    for (name, source, expected) in [
        (
            "suite uses",
            "suite demo.gen {\n  uses demo.a // first\n  uses demo.b\n\n  case \"x\" {\n    expect 1 == 1\n  }\n}\n",
            "\tuses demo.a  // first\n\n\tuses demo.b\n",
        ),
        (
            "store field",
            "context c\n\nagent Meter {\n  key id: String\n\n  store totals: Cell[Int] // c\n  store active: Cell[Bool]\n\n  on call get() -> Effect[Int] {\n    totals.get()\n  }\n}\n",
            "\tstore totals: Cell[Int]  // c\n\tstore active: Cell[Bool]\n",
        ),
        (
            "multi-line store field",
            "context c\n\nagent Meter {\n  key id: String\n\n  store active: Cell[Int] = match true {\n    true => 1\n    false => 0\n  } // c\n  store on: Cell[Bool]\n\n  on call get() -> Effect[Int] {\n    active.get()\n  }\n}\n",
            "\t}  // c\n\tstore on: Cell[Bool]\n",
        ),
    ] {
        let out = format_source(source, &FormatOptions::default())
            .unwrap_or_else(|e| panic!("{name}: refused: {}", e.errors[0].message));
        assert!(out.contains(expected), "{name}: spacing differs:\n{out}");
        let bare = source.replace(" // first", "").replace(" // c", "");
        let bare_out = format_source(&bare, &FormatOptions::default()).expect("formats");
        assert_eq!(
            out.lines().count(),
            bare_out.lines().count(),
            "{name}: the comment changed the line count:\n{out}"
        );
        assert_eq!(
            out,
            format_source(&out, &FormatOptions::default()).expect("reformats"),
            "{name}: not idempotent"
        );
    }
}

/// Format `source`, check the result reformats to itself, and return it.
fn format_idempotent(name: &str, source: &str) -> String {
    let out = format_source(source, &FormatOptions::default())
        .unwrap_or_else(|e| panic!("{name}: refused: {}", e.errors[0].message));
    assert_eq!(
        out,
        format_source(&out, &FormatOptions::default()).expect("reformats"),
        "{name}: not idempotent"
    );
    out
}

/// #1884: at unit level, a comment a blank line separates from the next
/// declaration stays separated, so it doesn't read as describing it. That
/// holds after the unit header, with or without a doc block below, and
/// between declarations. A run of blank lines keeps one.
#[test]
fn a_blank_line_after_a_unit_level_comment_is_kept() {
    for (name, source) in [
        (
            "after the header, above a doc block",
            "commons demo\n\n// What this module is for.\n\n---\nDoubles a number.\n---\nfn double(n: Int) -> Int {\n\tn * 2\n}\n",
        ),
        (
            "after the header, no doc block",
            "commons demo\n\n// What this module is for.\n\nfn double(n: Int) -> Int {\n\tn * 2\n}\n",
        ),
        (
            "between declarations",
            "commons demo\n\nfn double(n: Int) -> Int {\n\tn * 2\n}\n\n// A comment between declarations.\n\n---\nTriples a number.\n---\nfn triple(n: Int) -> Int {\n\tn * 3\n}\n",
        ),
        (
            "above a context's uses",
            "context demo\n\n// Dependencies.\n\nuses demo.a\n\nfn f() -> Int {\n\t1\n}\n",
        ),
        (
            "above the unit header",
            "// File header.\n\ncommons demo\n\nfn f() -> Int {\n\t1\n}\n",
        ),
    ] {
        let out = format_idempotent(name, source);
        assert_eq!(out, source, "{name}: the blank line moved");
    }
    let out = format_idempotent(
        "two blank lines",
        "commons demo\n\n// What this module is for.\n\n\nfn f() -> Int {\n\t1\n}\n",
    );
    assert_eq!(
        out,
        "commons demo\n\n// What this module is for.\n\nfn f() -> Int {\n\t1\n}\n"
    );
}

/// #1884's boundary: a comment directly above a declaration stays attached,
/// and in a body, where canonical style has no blank lines, a comment's blank
/// line goes as before.
#[test]
fn a_comment_without_a_blank_line_or_in_a_body_stays_attached() {
    let attached = "commons demo\n\n// Doubles.\nfn double(n: Int) -> Int {\n\tn * 2\n}\n";
    assert_eq!(format_idempotent("attached", attached), attached);
    for (name, source, expected) in [
        (
            "record field",
            "commons demo\n\ntype P = {\n  // the x\n\n  x: Int,\n  y: Int,\n}\n",
            "\t// the x\n\tx: Int,\n",
        ),
        (
            "statement",
            "commons demo\n\nfn f() -> Int {\n  // the answer\n\n  42\n}\n",
            "\t// the answer\n\t42\n",
        ),
        (
            // The comment on the `{` line leads the first item (#1788), but
            // the blank line below it was the `{`'s, not the comment's.
            "brace line",
            "suite demo.t { // the cases\n\n  case \"x\" {\n    expect 1 == 1\n  }\n}\n",
            "{\n\t// the cases\n\tcase \"x\" {\n",
        ),
        (
            "capability op",
            "context c\n\ncapability K {\n  fn a() -> Effect[Int]\n  // the b\n\n  fn b() -> Effect[Int]\n}\n",
            "\tfn a() -> Effect[Int]\n\t// the b\n\tfn b() -> Effect[Int]\n",
        ),
        (
            "cors field",
            "context api\n\nservice api from http {\n  cors {\n    // the origins\n\n    origins: [\"https://a.example.com\"],\n  }\n\n  on GET(\"/ping\") () -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"pong\")\n  }\n}\n",
            "\t\t// the origins\n\t\torigins: ",
        ),
        (
            "second store field",
            "context c\n\nagent Meter {\n  key id: String\n\n  store totals: Cell[Int]\n  // the flag\n\n  store active: Cell[Bool]\n\n  on call get() -> Effect[Int] {\n    totals.get()\n  }\n}\n",
            "\tstore totals: Cell[Int]\n\t// the flag\n\tstore active: Cell[Bool]\n",
        ),
    ] {
        let out = format_idempotent(name, source);
        assert!(
            out.contains(expected),
            "{name}: the blank line stayed:\n{out}"
        );
    }
}

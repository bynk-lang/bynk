use super::compact::has_multiline_template;
use super::decl::render_params;
use super::multiline::render_multiline_object;
use super::*;
use crate::program::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsDecl, TsDeclarator, TsExpr, TsLit, TsMappedModifier,
    TsObjectEntry, TsParam, TsProgram, TsStmt, TsStmtKind, TsType, TsTypeMember, TsUnaryOp,
};
use crate::program::{TsClassCtor, TsClassField, TsClassMethod, TsSwitchCase, VerbatimOrigin};
use crate::source_map::SourceMapBuilder;
use bynk_syntax::span::Span;

/// Pins the readability policy's statement-separation guarantee (R7.5,
/// this module's own doc): every statement starts on its own generated
/// line.
#[test]
fn prints_every_statement_in_order() {
    let mut program = TsProgram::new();
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Contracts,
        "const a = 1;\n",
        None,
    ));
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Secrets,
        "const b = 2;\n",
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "const a = 1;\nconst b = 2;\n");
}

/// Pins the readability policy's statement-separation guarantee (R7.5,
/// this module's own doc) in its sharpest form: nothing requires
/// `Verbatim` text to be newline-terminated — the printer owns line
/// structure (R7.3), so it's the one that guarantees two statements
/// never share a generated line, not a `TsStmt::verbatim` caller
/// obligation nobody enforces (review of #1308, finding 2).
#[test]
fn a_statement_missing_its_own_trailing_newline_still_gets_its_own_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Contracts,
        "const a = 1;",
        None,
    ));
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Secrets,
        "const b = 2;",
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "const a = 1;\nconst b = 2;\n");
}

/// Production `Verbatim` content is rarely one line — `VerbatimOrigin::
/// NotYetConverted` (P7.6) wraps a whole generated document per
/// statement, so one statement routinely spans dozens of generated
/// lines. Neither of the two tests above exercised that shape (review
/// of #1312, finding 1) — this one confirms the readability policy's
/// real invariant (the *next* statement always starts on a fresh line)
/// holds for multi-line content too, not just the single-line text
/// those tests happened to use.
#[test]
fn a_multi_line_statement_still_starts_the_next_statement_on_a_fresh_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Contracts,
        "function a() {\n  return 1;\n}\n",
        None,
    ));
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Secrets,
        "const b = 2;\n",
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "function a() {\n  return 1;\n}\nconst b = 2;\n"
    );
}

/// `print` decides whether to append a trailing newline by checking the
/// whole *buffer*, not the statement's own text (review of #1312,
/// finding 2) — an empty statement's text changes nothing, so whether
/// it gets a line depends entirely on what's already in the buffer.
/// First in a program, the buffer is still empty, so the check fires
/// and the output opens with a blank line. Pinned here so this edge
/// stays a known, deliberate reading of the code rather than an
/// untested accident.
#[test]
fn an_empty_statement_first_in_the_program_yields_a_leading_blank_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::verbatim(VerbatimOrigin::Contracts, "", None));
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Secrets,
        "const b = 2;\n",
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "\nconst b = 2;\n");
}

/// The other half of the buffer-vs-statement-text distinction (review
/// of #1312, finding 2): an empty statement following one that already
/// ended its own text in `\n` contributes no line at all — the buffer
/// already ends in `\n`, so the check doesn't fire, and the empty
/// statement's own (non-)content and the next statement's text end up
/// on what reads as one generated line for the next statement alone.
#[test]
fn an_empty_statement_after_a_newline_terminated_one_contributes_no_line_of_its_own() {
    let mut program = TsProgram::new();
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Contracts,
        "const a = 1;\n",
        None,
    ));
    program.push(TsStmt::verbatim(VerbatimOrigin::Secrets, "", None));
    program.push(TsStmt::verbatim(
        VerbatimOrigin::RuntimeUse,
        "const c = 3;\n",
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "const a = 1;\nconst c = 3;\n");
}

#[test]
fn no_spans_at_all_means_no_source_map() {
    let mut program = TsProgram::new();
    program.push(TsStmt::verbatim(VerbatimOrigin::Contracts, "x;\n", None));
    let printed = print(&program, "x.bynk", "let x = 1\n", "x.ts");
    assert_eq!(printed.source_map, None);
}

/// The property that actually matters (R7.4): the printer's own
/// buffer-position bookkeeping is correct — a checkpoint recorded from a
/// later statement's span still resolves to the *generated* line that
/// statement's own text landed on, not (as the old splice-based
/// mechanism could get wrong, #4) some other buffer's offset.
///
/// Review of #1308, finding 3: the original version of this test
/// asserted only that the source map's `sources` array was present —
/// true even with the second statement's checkpoint silently dropped,
/// exactly the failure this doc comment claims is ruled out. Asserting
/// full equality against a map built independently (replicating the
/// printer's own record-then-write order by hand, not calling `print`)
/// actually pins both statements' own offsets.
#[test]
fn each_statements_span_resolves_to_its_own_generated_line() {
    let source = "let a = 1\nlet b = 2\n";
    let off_a = source.find("let a").unwrap();
    let off_b = source.find("let b").unwrap();
    let span_a = Span::new(off_a, off_a + 5);
    let span_b = Span::new(off_b, off_b + 5);
    let text_a = "const a = 1;\n";
    let text_b = "const b = 2;\n";
    let mut program = TsProgram::new();
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Contracts,
        text_a,
        Some(span_a),
    ));
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Secrets,
        text_b,
        Some(span_b),
    ));

    let printed = print(&program, "x.bynk", source, "x.ts");

    let mut expected_map = SourceMapBuilder::new();
    expected_map.add_source("x.bynk", source);
    expected_map.record(0, span_a);
    expected_map.record(text_a.len(), span_b);
    let expected = expected_map.to_v3(&printed.text, "x.ts");

    assert_eq!(printed.source_map, expected);
}

// -- #1477: nested source-map checkpoints on an opaque body blob. --

/// `print_stmt_and_merge` — the `TsDecl::Function` shape (`emit_free_fn`'s
/// own real site): a `Raw` body statement's own `nested_map` merges into
/// the caller's map at the real print-time offset, appended into a
/// buffer that already has *other* content first (`base != 0`) — proving
/// this reads the caller's own real buffer position, not a fresh
/// throwaway one starting at 0 (the bug this function's own first draft
/// had, caught before merge: returning a `String` computes the offset
/// against that empty local buffer, silently wrong the moment the
/// caller splices the result anywhere but the very start of its own
/// `out`).
#[test]
fn print_stmt_and_merge_merges_a_function_bodys_own_nested_checkpoint() {
    let source = "fn f() { return 1 }\n";
    let ret_off = source.find("return 1").unwrap();
    let ret_span = Span::new(ret_off, ret_off + 8);

    let body_text = "  return 1;\n";
    let mut nested = SourceMapBuilder::new();
    nested.record(0, ret_span);

    let mut raw = TsStmt::raw(body_text, None);
    raw.nested_map = Some(nested);

    let func = TsStmt::decl(
        TsDecl::Function {
            name: "f".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body: vec![raw],
            is_async: false,
            inline: false,
        },
        None,
    );

    let mut out = String::from("// header\n");
    let mut map = SourceMapBuilder::new();
    map.add_source("x.bynk", source);

    print_stmt_and_merge(&mut out, &func, 0, &mut map, 0);

    assert_eq!(out, "// header\nfunction f() {\n  return 1;\n}\n");
    let base = out.find(body_text).unwrap();

    let mut expected = SourceMapBuilder::new();
    expected.add_source("x.bynk", source);
    expected.record(base, ret_span);
    assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
}

/// `print_class_method_and_merge` — the `TsClassMethod` shape
/// (`emit_provider`/`emit_agent`'s own real site, today reverse-engineered
/// by `emit_class_method_and_merge_source_map`'s own string-matching).
/// Same non-zero-`base` proof as the `TsDecl::Function` test above.
#[test]
fn print_class_method_and_merge_merges_a_methods_own_nested_checkpoint() {
    let source = "on call m() { commit }\n";
    let commit_off = source.find("commit").unwrap();
    let commit_span = Span::new(commit_off, commit_off + 6);

    let body_text = "    this.commit();\n";
    let mut nested = SourceMapBuilder::new();
    nested.record(0, commit_span);

    let mut raw = TsStmt::raw(body_text, None);
    raw.nested_map = Some(nested);

    let method = TsClassMethod {
        name: "m".to_string(),
        private: false,
        is_async: false,
        params: Vec::new(),
        return_type: None,
        doc: None,
        body: vec![raw],
        span: None,
    };

    let mut out = String::from("class C {\n");
    let mut map = SourceMapBuilder::new();
    map.add_source("x.bynk", source);

    print_class_method_and_merge(&mut out, &method, 0, &mut map, 0);

    let base = out.find(body_text).unwrap();
    let mut expected = SourceMapBuilder::new();
    expected.add_source("x.bynk", source);
    expected.record(base, commit_span);
    assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
}

/// `print()`'s own top-level loop (#1477): a real `TsProgram` containing
/// a `Decl`-kinded top-level statement whose own body carries a
/// `nested_map` merges it automatically — no `bynk-emit` change needed
/// to benefit once a real construction site sets the field, since
/// `print()` already threads its own module map through `render_stmt`
/// for every top-level statement.
#[test]
fn print_merges_a_top_level_functions_own_nested_checkpoint_automatically() {
    let source = "fn f() { return 1 }\n";
    let ret_off = source.find("return 1").unwrap();
    let ret_span = Span::new(ret_off, ret_off + 8);

    let body_text = "  return 1;\n";
    let mut nested = SourceMapBuilder::new();
    nested.record(0, ret_span);

    let mut raw = TsStmt::raw(body_text, None);
    raw.nested_map = Some(nested);

    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Function {
            name: "f".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body: vec![raw],
            is_async: false,
            inline: false,
        },
        None,
    ));

    let printed = print(&program, "x.bynk", source, "x.ts");
    let base = printed.text.find(body_text).unwrap();

    let mut expected = SourceMapBuilder::new();
    expected.add_source("x.bynk", source);
    expected.record(base, ret_span);
    assert_eq!(printed.source_map, expected.to_v3(&printed.text, "x.ts"));
}

/// Review of #1488, finding 1: `source_id` must reach the merge, not be
/// hardcoded — a multi-source module map (a test module aggregating
/// several `.bynk` files, `tests_emit.rs:537`/`:1936`'s own real shape)
/// tags a nested checkpoint to the *wrong* source silently if the merge
/// always assumes index 0.
///
/// Decisive by construction, not just an assertion on the map's own
/// `sources` list (which would pass either way — every registered
/// source name appears there regardless of which one a checkpoint
/// actually resolves against, `to_v3`'s own doc/body confirms `sources`/
/// `sourcesContent` iterate the full registered list unconditionally):
/// source 0's own text is a single short line, too short to contain the
/// real checkpoint's span. If the merge resolves the checkpoint against
/// source 0 (the bug this test exists to catch), `to_v3`'s own
/// out-of-range guard (`span.start > text.len()`) silently drops it and
/// the whole map comes back `None` (its only checkpoint). Only
/// resolving against the real source (id 1, correctly threaded through)
/// keeps the span in range and produces a real map.
#[test]
fn print_stmt_and_merge_tags_a_nested_checkpoint_to_the_given_source_id() {
    let primary = "x\n"; // deliberately shorter than `ret_span`'s own offset
    let secondary = "fn f() { return 1 }\n";
    let ret_off = secondary.find("return 1").unwrap();
    let ret_span = Span::new(ret_off, ret_off + 8);
    assert!(
        ret_span.start > primary.len(),
        "sanity: the span must fall outside source 0's own short text"
    );

    let body_text = "  return 1;\n";
    let mut nested = SourceMapBuilder::new();
    nested.record(0, ret_span);

    let mut raw = TsStmt::raw(body_text, None);
    raw.nested_map = Some(nested);

    let func = TsStmt::decl(
        TsDecl::Function {
            name: "f".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body: vec![raw],
            is_async: false,
            inline: false,
        },
        None,
    );

    let mut out = String::new();
    let mut map = SourceMapBuilder::new();
    map.add_source("primary.bynk", primary); // id 0
    let secondary_id = map.add_source("secondary.bynk", secondary); // id 1
    assert_eq!(secondary_id, 1);

    print_stmt_and_merge(&mut out, &func, 0, &mut map, secondary_id);

    assert!(
        map.to_v3(&out, "x.ts").is_some(),
        "the checkpoint resolved against the wrong source (id 0) and was silently dropped"
    );
}

/// #1486's own sibling gap to the one above, but in [`print_multi_
/// source`]'s own automatic top-level loop rather than the manual
/// `print_stmt_and_merge` entry point: a real `TsProgram` built for a
/// multi-source module (a test/integration module aggregating several
/// `.bynk` fragment files, one per case) needs each case's own
/// top-level statement merged against *its own* registered source, not
/// the module's primary one (id 0) — `nested_map_source_id`'s own doc
/// comment. Decisive the same way the sibling test above is: source 0's
/// text is too short to contain the real span, so a wrong-source merge
/// silently drops the checkpoint (`to_v3`'s own out-of-range guard) and
/// the map comes back missing it.
#[test]
fn print_multi_source_honours_a_top_level_statements_own_nested_map_source_id() {
    let primary = "x\n";
    let secondary = "fn f() { return 1 }\n";
    let ret_off = secondary.find("return 1").unwrap();
    let ret_span = Span::new(ret_off, ret_off + 8);
    assert!(
        ret_span.start > primary.len(),
        "sanity: the span must fall outside source 0's own short text"
    );

    let body_text = "  return 1;\n";
    let mut nested = SourceMapBuilder::new();
    nested.record(0, ret_span);

    let mut raw = TsStmt::raw(body_text, None);
    raw.nested_map = Some(nested);

    let mut func = TsStmt::decl(
        TsDecl::Function {
            name: "f".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body: vec![raw],
            is_async: false,
            inline: false,
        },
        None,
    );
    func.nested_map_source_id = 1;

    let mut program = TsProgram::new();
    program.push(func);

    let mut map = SourceMapBuilder::new();
    map.add_source("primary.bynk", primary);
    map.add_source("secondary.bynk", secondary);
    let printed = print_multi_source(&program, map, "x.ts");
    assert!(
        printed.source_map.is_some(),
        "the checkpoint resolved against the wrong source (id 0) and was silently dropped"
    );
}

/// Review of #1488, finding 2: a `nested_map` set on the statement handed
/// directly to `print`/`print_stmt_and_merge` itself — not nested inside
/// a `Decl` body — must still merge, not silently vanish. The first
/// version of this mechanism only checked a body's own *direct
/// children* (inside `render_block_stmts`), missing this exact case.
#[test]
fn print_stmt_and_merge_merges_a_bare_top_level_raw_statements_own_nested_checkpoint() {
    let source = "contract seal() {}\n";
    let off = source.find("contract").unwrap();
    let span = Span::new(off, off + 8);

    let text = "seal();\n";
    let mut nested = SourceMapBuilder::new();
    nested.record(0, span);

    let mut raw = TsStmt::raw(text, None);
    raw.nested_map = Some(nested);

    let mut out = String::from("// header\n");
    let mut map = SourceMapBuilder::new();
    map.add_source("x.bynk", source);

    print_stmt_and_merge(&mut out, &raw, 0, &mut map, 0);

    let base = out.find(text).unwrap();
    let mut expected = SourceMapBuilder::new();
    expected.add_source("x.bynk", source);
    expected.record(base, span);
    assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
}

/// Review of #1488, finding 3 (part 1): the merge's own line-rebasing
/// arithmetic (`out_line = base_line + body_line`) is exercised for
/// real, not just the degenerate `body_line == 0` case every other test
/// here uses — a two-statement body with the checkpoint on the
/// *second* line.
#[test]
fn print_stmt_and_merge_rebases_a_checkpoint_on_a_bodys_second_line() {
    let source = "fn f() { let a = 1\n  return a }\n";
    let ret_off = source.find("return a").unwrap();
    let ret_span = Span::new(ret_off, ret_off + 8);

    let body_text = "  const a = 1;\n  return a;\n";
    let second_line_off = body_text.find("return a;").unwrap();
    let mut nested = SourceMapBuilder::new();
    nested.record(second_line_off, ret_span);

    let mut raw = TsStmt::raw(body_text, None);
    raw.nested_map = Some(nested);

    let func = TsStmt::decl(
        TsDecl::Function {
            name: "f".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body: vec![raw],
            is_async: false,
            inline: false,
        },
        None,
    );

    let mut out = String::from("// header\n");
    let mut map = SourceMapBuilder::new();
    map.add_source("x.bynk", source);

    print_stmt_and_merge(&mut out, &func, 0, &mut map, 0);

    let base_of_body = out.find(body_text).unwrap();
    let base_of_second_line = out.find("  return a;\n").unwrap();
    assert!(
        base_of_second_line > base_of_body,
        "sanity: second line is after the first"
    );

    let mut expected = SourceMapBuilder::new();
    expected.add_source("x.bynk", source);
    expected.record(base_of_second_line, ret_span);
    assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
}

/// Review of #1488, finding 3 (part 2): `TsDecl::Class`'s own
/// constructor-then-methods sequence is the one real place `map` is
/// reborrowed across more than one consumer — a class with a
/// `nested_map`-bearing constructor body *and* a `nested_map`-bearing
/// method body proves both reborrows land correctly, not just the
/// first one. `emit_provider`/`emit_agent` (both class-shaped) are
/// exactly the real callers this slice unblocks.
#[test]
fn a_classs_constructor_and_method_both_merge_their_own_nested_checkpoints() {
    let source = "on new() { init }\non call m() { commit }\n";
    let init_off = source.find("init").unwrap();
    let init_span = Span::new(init_off, init_off + 4);
    let commit_off = source.find("commit").unwrap();
    let commit_span = Span::new(commit_off, commit_off + 6);

    let ctor_body_text = "    this.init();\n";
    let mut ctor_nested = SourceMapBuilder::new();
    ctor_nested.record(0, init_span);
    let mut ctor_raw = TsStmt::raw(ctor_body_text, None);
    ctor_raw.nested_map = Some(ctor_nested);

    let method_body_text = "    this.commit();\n";
    let mut method_nested = SourceMapBuilder::new();
    method_nested.record(0, commit_span);
    let mut method_raw = TsStmt::raw(method_body_text, None);
    method_raw.nested_map = Some(method_nested);

    let class = TsDecl::Class {
        name: "C".to_string(),
        implements: Vec::new(),
        fields: Vec::new(),
        constructor: Some(TsClassCtor {
            params: Vec::new(),
            body: vec![ctor_raw],
        }),
        methods: vec![TsClassMethod {
            name: "m".to_string(),
            private: false,
            is_async: false,
            params: Vec::new(),
            return_type: None,
            doc: None,
            body: vec![method_raw],
            span: None,
        }],
        end_span: None,
    };

    let mut out = String::new();
    let mut map = SourceMapBuilder::new();
    map.add_source("x.bynk", source);

    print_stmt_and_merge(&mut out, &TsStmt::decl(class, None), 0, &mut map, 0);

    let ctor_base = out.find(ctor_body_text).unwrap();
    let method_base = out.find(method_body_text).unwrap();

    let mut expected = SourceMapBuilder::new();
    expected.add_source("x.bynk", source);
    expected.record(ctor_base, init_span);
    expected.record(method_base, commit_span);
    assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
}

// -- P7.8 (#1313): real node rendering. --

#[test]
fn prints_a_const_statement_with_a_destructured_binding_and_a_type_cast() {
    let mut program = TsProgram::new();
    program.push(TsStmt::const_stmt(
        TsBindingName::ObjectPattern(vec!["events".to_string()]),
        None,
        TsExpr::As {
            // #1323: `Await` no longer auto-parenthesizes under `As` —
            // this file's own real content wraps it explicitly via
            // `Paren` (see `render_expr`'s own `As` arm doc).
            expr: Box::new(TsExpr::Paren(Box::new(TsExpr::Await(Box::new(
                TsExpr::Call {
                    callee: Box::new(TsExpr::Member {
                        object: Box::new(TsExpr::Ident("request".to_string())),
                        property: "json".to_string(),
                    }),
                    args: vec![],
                },
            ))))),
            ty: TsType::Object(vec![TsTypeMember::prop(
                "events",
                TsType::array(TsType::named("FanoutEvent")),
            )]),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "const { events } = (await request.json()) as { events: FanoutEvent[] };\n"
    );
}

#[test]
fn prints_if_without_braces_when_the_body_is_not_a_block() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_stmt(
        TsExpr::Unary {
            op: TsUnaryOp::Not,
            expr: Box::new(TsExpr::Call {
                callee: Box::new(TsExpr::Member {
                    object: Box::new(TsExpr::Ident("Array".to_string())),
                    property: "isArray".to_string(),
                }),
                args: vec![TsExpr::Ident("subs".to_string())],
            }),
        },
        TsStmt::continue_stmt(None),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "if (!Array.isArray(subs)) continue;\n");
}

/// Review of #1317/#1318, finding 2: a bare `Comment` as an `if`'s
/// brace-free body must not fall through to the top-level `//`-line
/// form — `if (cond) // text` comments out the rest of the physical
/// line, leaving `if` with no body at all (a parse error). Renders as
/// a block comment instead, which cannot swallow anything after it.
#[test]
fn a_comment_as_an_ifs_brace_free_body_renders_as_a_block_comment() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_stmt(
        TsExpr::Ident("cond".to_string()),
        TsStmt::comment("annotation", None),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "if (cond) /* annotation */\n");
}

#[test]
fn prints_a_for_of_loop_with_a_braced_body() {
    let mut program = TsProgram::new();
    program.push(TsStmt::for_of(
        "ev",
        TsExpr::Ident("events".to_string()),
        TsStmt::block(
            vec![TsStmt::return_stmt(
                Some(TsExpr::Ident("ev".to_string())),
                None,
            )],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "for (const ev of events) {\n  return ev;\n}\n"
    );
}

/// Arc E slice 7 (#1447): `TsStmtKind::For` — pins the exact C-style
/// header shape `ListInst`/`MapInst`'s own real deserialise-side element
/// loops need (`for (let i = 0; i < json.length; i++) { ... }`),
/// including the postfix `++` the printer appends over `name` itself
/// (see `TsStmtKind::For`'s own doc for why there is no separate
/// `update` field to pass).
#[test]
fn prints_a_c_style_for_loop_with_a_braced_body() {
    let mut program = TsProgram::new();
    program.push(TsStmt::for_stmt(
        "i",
        TsExpr::Lit(TsLit::Num("0".to_string())),
        TsExpr::Binary {
            op: TsBinaryOp::LessThan,
            left: Box::new(TsExpr::Ident("i".to_string())),
            right: Box::new(TsExpr::Member {
                object: Box::new(TsExpr::Ident("json".to_string())),
                property: "length".to_string(),
            }),
        },
        TsStmt::block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("item".to_string()), None)],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "for (let i = 0; i < json.length; i++) {\n  item;\n}\n"
    );
}

/// A `For` loop's own brace-free body falls back to the same inline
/// rendering `ForOf`'s own brace-free body already uses — exercised as
/// the *outer* statement, over a `Continue` body (not reachable from any
/// real `bynk-emit` content today, but a real, direct shape).
#[test]
fn prints_a_c_style_for_loops_brace_free_body_inline() {
    let mut program = TsProgram::new();
    program.push(TsStmt::for_stmt(
        "i",
        TsExpr::Lit(TsLit::Num("0".to_string())),
        TsExpr::Binary {
            op: TsBinaryOp::LessThan,
            left: Box::new(TsExpr::Ident("i".to_string())),
            right: Box::new(TsExpr::Ident("n".to_string())),
        },
        TsStmt::continue_stmt(None),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "for (let i = 0; i < n; i++) continue;\n");
}

/// Review of #1448, finding 1: the previous test above pins a brace-free
/// `For` as the *outer* statement, which renders through `render_stmt`'s
/// own dedicated `For` arm, not the `render_inline_stmt` fallback this
/// slice's own `| TsStmtKind::For { .. }` line actually added. That
/// fallback only fires when a `For` is itself the brace-free body of an
/// ENCLOSING `if`/`for...of`/`For` — exercised directly here, nesting a
/// brace-free `For` as an `if`'s own brace-free branch.
#[test]
fn prints_a_brace_free_for_loop_nested_as_an_ifs_own_brace_free_body() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_stmt(
        TsExpr::Ident("cond".to_string()),
        TsStmt::for_stmt(
            "i",
            TsExpr::Lit(TsLit::Num("0".to_string())),
            TsExpr::Binary {
                op: TsBinaryOp::LessThan,
                left: Box::new(TsExpr::Ident("i".to_string())),
                right: Box::new(TsExpr::Ident("n".to_string())),
            },
            TsStmt::continue_stmt(None),
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "if (cond) for (let i = 0; i < n; i++) continue;\n"
    );
}

#[test]
fn prints_a_try_catch_on_the_shared_closing_brace_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::try_catch(
        TsStmt::block(
            vec![TsStmt::expr_stmt(
                TsExpr::Await(Box::new(TsExpr::Call {
                    callee: Box::new(TsExpr::Ident("deliverEvent".to_string())),
                    args: vec![TsExpr::Ident("binding".to_string())],
                })),
                None,
            )],
            None,
        ),
        Some("e"),
        TsStmt::block(
            vec![TsStmt::expr_stmt(
                TsExpr::Call {
                    callee: Box::new(TsExpr::Member {
                        object: Box::new(TsExpr::Ident("console".to_string())),
                        property: "error".to_string(),
                    }),
                    args: vec![TsExpr::Lit(TsLit::Str("failed".to_string()))],
                },
                None,
            )],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "try {\n  await deliverEvent(binding);\n} catch (e) {\n  console.error(\"failed\");\n}\n"
    );
}

#[test]
fn prints_an_interface_with_an_inline_nested_object_type() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Interface {
            name: "FanoutEvent".to_string(),
            type_params: Vec::new(),
            members: vec![
                TsTypeMember::prop("type", TsType::named("string")),
                TsTypeMember::prop("payload", TsType::named("unknown")),
                TsTypeMember::prop(
                    "envelope",
                    TsType::Object(vec![
                        TsTypeMember::prop("eventId", TsType::named("string")),
                        TsTypeMember::prop("publisherId", TsType::named("string")),
                    ]),
                ),
            ],
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "interface FanoutEvent {\n  type: string;\n  payload: unknown;\n  envelope: { eventId: string; publisherId: string };\n}\n"
    );
}

#[test]
fn prints_two_adjacent_imports_with_no_blank_line_between_them() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Import {
            type_only: true,
            names: vec!["A".to_string()],
            from: "../x.js".to_string(),
        },
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::Import {
            type_only: false,
            names: vec!["b".to_string()],
            from: "../x.js".to_string(),
        },
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::ConstDecl {
            name: "c".to_string(),
            ty: None,
            init: TsExpr::Lit(TsLit::Null),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "import type { A } from \"../x.js\";\nimport { b } from \"../x.js\";\n\nconst c = null;\n"
    );
}

/// #1486: `no_blank_before` suppresses the automatic blank line before
/// exactly the statement that carries it — a `const`/`function` pair
/// neither an import, a comment, a `ReExportAll`, nor `Verbatim`, so
/// none of the existing exemptions apply; without this field the two
/// would print with an unwanted blank between them (confirmed by
/// temporarily reverting the field during this same change — the
/// printer's own default policy really does insert one here). A third,
/// ordinary statement after it confirms the field is scoped to the one
/// carrying it, not sticky for the rest of the program.
#[test]
fn no_blank_before_suppresses_exactly_the_next_statements_own_blank() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ConstDecl {
            name: "__FooRegistry".to_string(),
            ty: None,
            init: TsExpr::Lit(TsLit::Null),
        },
        None,
    ));
    let mut zero_fn = TsStmt::decl(
        TsDecl::Function {
            name: "__zeroOfFooState".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body: Vec::new(),
            is_async: false,
            inline: true,
        },
        None,
    );
    zero_fn.no_blank_before = true;
    program.push(zero_fn);
    program.push(TsStmt::decl(
        TsDecl::ConstDecl {
            name: "c".to_string(),
            ty: None,
            init: TsExpr::Lit(TsLit::Null),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "const __FooRegistry = null;\nfunction __zeroOfFooState() {  }\n\nconst c = null;\n"
    );
}

/// The required "does the algebra hold against real content" test
/// (P7.8's own accepted proposal, #1313): builds
/// `bynk-emit/src/emitter/events_fanout.rs`'s own `EventsFanoutDO`
/// class as real nodes (its field, constructor, and `fetch` method,
/// with the exact control flow the real function emits — `for...of`,
/// nested `if`-without-braces, `try`/`catch`) and asserts the printed
/// text is byte-identical to what `emit_events_fanout_do` produces
/// today for that class (transcribed directly from
/// `bynk-emit/src/emitter/events_fanout.rs`'s own `write!` calls, not
/// re-derived — including the constructor body, `this.env = (env ??
/// {}) as Record<string, ServiceBinding>;`, via `TsStmtKind::Assign`,
/// added in review of #1313: the accepted proposal's own grounding
/// catalogue detailed the `fetch` method's body but missed the
/// constructor's one real statement, which turned out to need a
/// variant Decision B's list didn't name). The leading header comment
/// and the `__eventRoutes` table's own per-project literal entries are
/// deliberately not included here — a representative fragment covering
/// the class is exactly what the accepted proposal's own "Done when"
/// allows, and this is the shape that actually exercises every new
/// node kind this slice adds.
#[test]
fn prints_events_fanout_dos_own_class_byte_identical_to_the_real_emitter() {
    let real_ctor = TsClassCtor {
        params: vec![
            TsParam::new(
                "_state".to_string(),
                Some(TsType::named("DurableObjectState")),
            ),
            TsParam::optional("env".to_string(), Some(TsType::named("unknown"))),
        ],
        body: vec![TsStmt::assign(
            TsExpr::Member {
                object: Box::new(TsExpr::Ident("this".to_string())),
                property: "env".to_string(),
            },
            TsExpr::As {
                expr: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::NullishCoalescing,
                    left: Box::new(TsExpr::Ident("env".to_string())),
                    right: Box::new(TsExpr::object(vec![])),
                }),
                ty: TsType::named_with_args(
                    "Record",
                    vec![TsType::named("string"), TsType::named("ServiceBinding")],
                ),
            },
            None,
        )],
    };

    let fetch = TsClassMethod {
        name: "fetch".to_string(),
        private: false,
        is_async: true,
        params: vec![TsParam::new(
            "request".to_string(),
            Some(TsType::named("Request")),
        )],
        return_type: Some(TsType::named_with_args(
            "Promise",
            vec![TsType::named("Response")],
        )),
        doc: None,
        body: vec![
            TsStmt::const_stmt(
                TsBindingName::ObjectPattern(vec!["events".to_string()]),
                None,
                TsExpr::As {
                    expr: Box::new(TsExpr::Paren(Box::new(TsExpr::Await(Box::new(
                        TsExpr::Call {
                            callee: Box::new(TsExpr::Member {
                                object: Box::new(TsExpr::Ident("request".to_string())),
                                property: "json".to_string(),
                            }),
                            args: vec![],
                        },
                    ))))),
                    ty: TsType::Object(vec![TsTypeMember::prop(
                        "events",
                        TsType::array(TsType::named("FanoutEvent")),
                    )]),
                },
                None,
            ),
            TsStmt::for_of(
                "ev",
                TsExpr::Ident("events".to_string()),
                TsStmt::block(
                    vec![
                        TsStmt::const_stmt(
                            TsBindingName::Ident("subs".to_string()),
                            None,
                            TsExpr::Index {
                                object: Box::new(TsExpr::Ident("__eventRoutes".to_string())),
                                index: Box::new(TsExpr::Member {
                                    object: Box::new(TsExpr::Ident("ev".to_string())),
                                    property: "type".to_string(),
                                }),
                            },
                            None,
                        ),
                        TsStmt::if_stmt(
                            TsExpr::Unary {
                                op: TsUnaryOp::Not,
                                expr: Box::new(TsExpr::Call {
                                    callee: Box::new(TsExpr::Member {
                                        object: Box::new(TsExpr::Ident("Array".to_string())),
                                        property: "isArray".to_string(),
                                    }),
                                    args: vec![TsExpr::Ident("subs".to_string())],
                                }),
                            },
                            TsStmt::continue_stmt(None),
                            None,
                        ),
                        TsStmt::for_of(
                            "sub",
                            TsExpr::Ident("subs".to_string()),
                            TsStmt::block(
                                vec![
                                    TsStmt::const_stmt(
                                        TsBindingName::Ident("binding".to_string()),
                                        None,
                                        TsExpr::Index {
                                            object: Box::new(TsExpr::Member {
                                                object: Box::new(TsExpr::Ident("this".to_string())),
                                                property: "env".to_string(),
                                            }),
                                            index: Box::new(TsExpr::Member {
                                                object: Box::new(TsExpr::Ident("sub".to_string())),
                                                property: "binding".to_string(),
                                            }),
                                        },
                                        None,
                                    ),
                                    TsStmt::if_stmt(
                                        TsExpr::Unary {
                                            op: TsUnaryOp::Not,
                                            expr: Box::new(TsExpr::Ident("binding".to_string())),
                                        },
                                        TsStmt::continue_stmt(None),
                                        None,
                                    ),
                                    TsStmt::try_catch(
                                        TsStmt::block(
                                            vec![TsStmt::expr_stmt(
                                                TsExpr::Await(Box::new(TsExpr::Call {
                                                    callee: Box::new(TsExpr::Ident(
                                                        "deliverEvent".to_string(),
                                                    )),
                                                    args: vec![
                                                        TsExpr::Ident("binding".to_string()),
                                                        TsExpr::Member {
                                                            object: Box::new(TsExpr::Ident(
                                                                "sub".to_string(),
                                                            )),
                                                            property: "service".to_string(),
                                                        },
                                                        TsExpr::Member {
                                                            object: Box::new(TsExpr::Ident(
                                                                "ev".to_string(),
                                                            )),
                                                            property: "payload".to_string(),
                                                        },
                                                        TsExpr::Member {
                                                            object: Box::new(TsExpr::Ident(
                                                                "ev".to_string(),
                                                            )),
                                                            property: "envelope".to_string(),
                                                        },
                                                    ],
                                                })),
                                                None,
                                            )],
                                            None,
                                        ),
                                        Some("e"),
                                        TsStmt::block(
                                            vec![TsStmt::expr_stmt(
                                                TsExpr::Call {
                                                    callee: Box::new(TsExpr::Member {
                                                        object: Box::new(TsExpr::Ident(
                                                            "console".to_string(),
                                                        )),
                                                        property: "error".to_string(),
                                                    }),
                                                    args: vec![
                                                        TsExpr::Lit(TsLit::Str(
                                                            "EventsFanout delivery failed"
                                                                .to_string(),
                                                        )),
                                                        TsExpr::object(vec![
                                                            (
                                                                "event".to_string(),
                                                                TsExpr::Member {
                                                                    object: Box::new(
                                                                        TsExpr::Ident(
                                                                            "ev".to_string(),
                                                                        ),
                                                                    ),
                                                                    property: "type".to_string(),
                                                                },
                                                            ),
                                                            (
                                                                "service".to_string(),
                                                                TsExpr::Member {
                                                                    object: Box::new(
                                                                        TsExpr::Ident(
                                                                            "sub".to_string(),
                                                                        ),
                                                                    ),
                                                                    property: "service".to_string(),
                                                                },
                                                            ),
                                                            (
                                                                "error".to_string(),
                                                                TsExpr::Call {
                                                                    callee: Box::new(
                                                                        TsExpr::Ident(
                                                                            "String".to_string(),
                                                                        ),
                                                                    ),
                                                                    args: vec![TsExpr::Ident(
                                                                        "e".to_string(),
                                                                    )],
                                                                },
                                                            ),
                                                        ]),
                                                    ],
                                                },
                                                None,
                                            )],
                                            None,
                                        ),
                                        None,
                                    ),
                                ],
                                None,
                            ),
                            None,
                        ),
                    ],
                    None,
                ),
                None,
            ),
            TsStmt::return_stmt(
                Some(TsExpr::New {
                    callee: Box::new(TsExpr::Ident("Response".to_string())),
                    args: vec![
                        TsExpr::Lit(TsLit::Null),
                        TsExpr::object(vec![(
                            "status".to_string(),
                            TsExpr::Lit(TsLit::Num("204".to_string())),
                        )]),
                    ],
                }),
                None,
            ),
        ],
        span: None,
    };

    let class = TsDecl::Export(Box::new(TsDecl::Class {
        name: "EventsFanoutDO".to_string(),
        implements: Vec::new(),
        fields: vec![
            TsClassField::new(
                "env",
                TsType::named_with_args(
                    "Record",
                    vec![TsType::named("string"), TsType::named("ServiceBinding")],
                ),
            )
            .private(),
        ],
        constructor: Some(real_ctor),
        methods: vec![fetch],
        end_span: None,
    }));

    let mut program = TsProgram::new();
    program.push(TsStmt::decl(class, None));
    let printed = print(&program, "x.bynk", "", "x.ts");

    // Transcribed directly from `bynk-emit/src/emitter/events_fanout.rs`'s
    // own `write!` calls, byte-for-byte including the constructor body.
    let expected_lines = [
        "export class EventsFanoutDO {",
        "  private env: Record<string, ServiceBinding>;",
        "  constructor(_state: DurableObjectState, env?: unknown) {",
        "    this.env = (env ?? {}) as Record<string, ServiceBinding>;",
        "  }",
        "",
        "  async fetch(request: Request): Promise<Response> {",
        "    const { events } = (await request.json()) as { events: FanoutEvent[] };",
        "    for (const ev of events) {",
        "      const subs = __eventRoutes[ev.type];",
        "      if (!Array.isArray(subs)) continue;",
        "      for (const sub of subs) {",
        "        const binding = this.env[sub.binding];",
        "        if (!binding) continue;",
        "        try {",
        "          await deliverEvent(binding, sub.service, ev.payload, ev.envelope);",
        "        } catch (e) {",
        "          console.error(\"EventsFanout delivery failed\", { event: ev.type, service: sub.service, error: String(e) });",
        "        }",
        "      }",
        "    }",
        "    return new Response(null, { status: 204 });",
        "  }",
        "}",
    ];
    let expected = format!("{}\n", expected_lines.join("\n"));
    assert_eq!(printed.text, expected);
}

/// Coverage gap named in review of #1314, finding 4: `Let` had no test
/// anywhere, including its distinct `init: None` branch (`Const` always
/// has an initialiser; `Let` is the one new statement kind that can
/// omit one).
#[test]
fn prints_a_let_statement_with_no_initialiser() {
    let mut program = TsProgram::new();
    program.push(TsStmt::let_stmt(
        TsBindingName::Ident("subs".to_string()),
        None,
        None,
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "let subs;\n");
}

/// Coverage gap named in review of #1314, finding 4: `TsExpr::Array`
/// was never constructed in any test.
#[test]
fn prints_an_array_literal() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::array(vec![
            TsExpr::Lit(TsLit::Num("1".to_string())),
            TsExpr::Ident("x".to_string()),
        ]),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "[1, x];\n");
}

/// P7.9 (#1315): `TsType::Array`'s new `readonly` modifier — the shape
/// every `List`/`Query` element type `bynk-emit`'s `ts_type_ref*`/
/// `ts_ty` families build (`readonly T[]`), which `TsType::array`'s own
/// `readonly: false` default cannot represent.
#[test]
fn print_type_renders_a_readonly_array() {
    assert_eq!(
        print_type(&TsType::readonly_array(TsType::named("Order"))),
        "readonly Order[]"
    );
    assert_eq!(
        print_type(&TsType::array(TsType::named("Order"))),
        "Order[]"
    );
}

/// A readonly array, function, union or intersection element is
/// parenthesised before `[]`; a bare `readonly readonly T[][]` is TS1354.
#[test]
fn print_type_parenthesises_a_non_postfix_safe_array_element() {
    let order = || TsType::named("Order");
    assert_eq!(
        print_type(&TsType::readonly_array(TsType::readonly_array(order()))),
        "readonly (readonly Order[])[]"
    );
    assert_eq!(
        print_type(&TsType::readonly_array(TsType::array(order()))),
        "readonly Order[][]"
    );
    assert_eq!(
        print_type(&TsType::readonly_array(TsType::Fn {
            rest: false,
            names: vec![],
            params: vec![order()],
            ret: Box::new(order()),
        })),
        "readonly ((a0: Order) => Order)[]"
    );
    assert_eq!(
        print_type(&TsType::array(TsType::Union {
            members: vec![order(), TsType::named("null")],
            multiline: false,
        })),
        "(Order | null)[]"
    );
    assert_eq!(
        print_type(&TsType::array(TsType::Intersection(vec![
            order(),
            TsType::named("Tagged"),
        ]))),
        "(Order & Tagged)[]"
    );
}

/// The mapped type, with each of its three `readonly` forms.
#[test]
fn print_type_renders_a_mapped_type_with_each_readonly_modifier() {
    let mapped = || {
        TsType::mapped(
            "K",
            TsType::named("S"),
            TsType::indexed(TsType::named("S"), TsType::named("K")),
        )
    };
    assert_eq!(print_type(&mapped()), "{ [K in keyof S]: S[K] }");
    assert_eq!(
        print_type(&mapped().with_mapped_readonly(TsMappedModifier::Remove)),
        "{ -readonly [K in keyof S]: S[K] }"
    );
    assert_eq!(
        print_type(&mapped().with_mapped_readonly(TsMappedModifier::Add)),
        "{ +readonly [K in keyof S]: S[K] }"
    );
}

/// `keyof` binds tighter than a union, so a union operand is parenthesised.
#[test]
fn print_type_parenthesises_a_union_under_keyof() {
    let ty = TsType::mapped(
        "P",
        TsType::union(vec![TsType::named("A"), TsType::named("B")]),
        TsType::named("unknown"),
    );
    assert_eq!(print_type(&ty), "{ [P in keyof (A | B)]: unknown }");
}

/// `typeof x` in type position, bare under `keyof` and parenthesised before an
/// index: the shape the agent rehydration check writes through.
#[test]
fn print_type_renders_typeof_and_indexed_access() {
    let of_s = || TsType::type_of(TsExpr::Ident("s".to_string()));
    assert_eq!(print_type(&of_s()), "typeof s");
    assert_eq!(
        print_type(&TsType::indexed(of_s(), TsType::string_literal("n"))),
        "(typeof s)[\"n\"]"
    );
    assert_eq!(
        print_type(
            &TsType::mapped("K", of_s(), TsType::indexed(of_s(), TsType::named("K")),)
                .with_mapped_readonly(TsMappedModifier::Remove)
        ),
        "{ -readonly [K in keyof typeof s]: (typeof s)[K] }"
    );
    assert_eq!(
        print_type(&TsType::type_of(TsExpr::Member {
            object: Box::new(TsExpr::Ident("a".to_string())),
            property: "b".to_string(),
        })),
        "typeof a.b"
    );
}

/// An indexed access on a named type is bare, and a string literal type escapes
/// as a string literal does.
#[test]
fn print_type_renders_an_indexed_access_on_a_named_type() {
    assert_eq!(
        print_type(&TsType::indexed(
            TsType::named("S"),
            TsType::string_literal("a\"b"),
        )),
        "S[\"a\\\"b\"]"
    );
    assert_eq!(
        print_type(&TsType::indexed(
            TsType::union(vec![TsType::named("A"), TsType::named("B")]),
            TsType::named("K"),
        )),
        "(A | B)[K]"
    );
}

/// `TsType::Paren` always prints its parentheses, around whatever it holds (the thunk
/// type a `Query` annotation is written as), and an array of it is not parenthesised
/// again.
#[test]
fn print_type_renders_a_parenthesised_type() {
    let thunk = TsType::Fn {
        rest: false,
        names: vec![],
        params: vec![],
        ret: Box::new(TsType::named("T")),
    };
    assert_eq!(print_type(&TsType::paren(thunk.clone())), "(() => T)");
    assert_eq!(
        print_type(&TsType::array(TsType::paren(thunk))),
        "(() => T)[]"
    );
    assert_eq!(print_type(&TsType::paren(TsType::named("A"))), "(A)");
}

/// A parameter that is both optional and defaulted prints `x?: T = v`, a TypeScript
/// error, so a struct literal that builds one is refused as `with_default` refuses it.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "cannot be optional and have a default")]
fn an_optional_parameter_with_a_default_is_refused() {
    let mut p = TsParam::new("x", Some(TsType::named("T")));
    p.optional = true;
    p.default = Some(TsExpr::Lit(TsLit::Num("1".to_string())));
    let mut out = String::new();
    render_params(&mut out, &[p]);
}

/// A `bigint` literal prints its digits and the `n` suffix, whatever its size or sign.
#[test]
fn print_expr_renders_a_bigint_literal() {
    assert_eq!(print_expr(&TsExpr::bigint(0)), "0n");
    assert_eq!(print_expr(&TsExpr::bigint(-5)), "-5n");
    assert_eq!(
        print_expr(&TsExpr::bigint(i128::MIN)),
        "-170141183460469231731687303715884105728n"
    );
    let wide = "1".repeat(41);
    assert_eq!(print_expr(&TsExpr::bigint(&wide)), format!("{wide}n"));
}

/// An integer literal type indexes a tuple type: `Parameters<typeof f>[0]`.
#[test]
fn print_type_renders_a_number_literal_index() {
    assert_eq!(print_type(&TsType::number_literal(7)), "7");
    let params = TsType::named_with_args(
        "Parameters",
        vec![TsType::type_of(TsExpr::Ident("f".to_string()))],
    );
    assert_eq!(
        print_type(&TsType::indexed(params, TsType::number_literal(0))),
        "Parameters<typeof f>[0]"
    );
}

/// P7.9 (#1315): `TsType::Fn`'s zero-`params` form — the query-thunk
/// wrapper shape, `bynk-emit`'s own `(() => readonly T[])`.
#[test]
fn print_type_renders_a_zero_param_function_type() {
    let ty = TsType::Fn {
        rest: false,
        names: vec![],
        params: vec![],
        ret: Box::new(TsType::readonly_array(TsType::named("Order"))),
    };
    assert_eq!(print_type(&ty), "() => readonly Order[]");
}

/// P7.9 (#1315): `TsType::Fn`'s real parametered form — positional
/// `a0`/`a1`/… names, matching `bynk-emit`'s own pre-P7.9 convention
/// exactly (TypeScript requires *some* name in function-type syntax).
#[test]
fn print_type_renders_a_parametered_function_type_with_positional_names() {
    let ty = TsType::Fn {
        rest: false,
        names: vec![],
        params: vec![TsType::named("string"), TsType::named("number")],
        ret: Box::new(TsType::named("void")),
    };
    assert_eq!(print_type(&ty), "(a0: string, a1: number) => void");
}

/// 1A.40c (#3996): `rest` makes the last parameter a rest parameter, `...name: T`, which the
/// agent method table's `(...bynkArgs: unknown[]) => unknown` needs; only the last one takes
/// the `...`, and `rest: false` is every other function type, unchanged.
#[test]
fn print_type_renders_a_rest_parameter_on_the_last_parameter_only() {
    let rest = TsType::Fn {
        rest: true,
        names: vec!["first".to_string(), "bynkArgs".to_string()],
        params: vec![
            TsType::named("string"),
            TsType::array(TsType::named("unknown")),
        ],
        ret: Box::new(TsType::named("unknown")),
    };
    assert_eq!(
        print_type(&rest),
        "(first: string, ...bynkArgs: unknown[]) => unknown"
    );
    let plain = TsType::Fn {
        rest: false,
        names: vec!["first".to_string(), "bynkArgs".to_string()],
        params: vec![
            TsType::named("string"),
            TsType::array(TsType::named("unknown")),
        ],
        ret: Box::new(TsType::named("unknown")),
    };
    assert_eq!(
        print_type(&plain),
        "(first: string, bynkArgs: unknown[]) => unknown"
    );
}

/// `TsType::Union`, added during review of #1315's own implementation —
/// a real gap `bynk-emit`'s `ts_ty` needed for a resolved multi-actor
/// sum (`Ty::ActorSum`), beyond the accepted proposal's own `readonly`/
/// `Fn` gap list. Members print `" | "`-joined, in order.
#[test]
fn print_type_renders_a_union_of_named_types() {
    let ty = TsType::union(vec![
        TsType::named("string"),
        TsType::named("number"),
        TsType::Object(vec![TsTypeMember::prop(
            "tag",
            TsType::named("\"literal\""),
        )]),
    ]);
    assert_eq!(print_type(&ty), "string | number | { tag: \"literal\" }");
}

/// `TsStmtKind::Comment`, added for Arc C's own first real conversion
/// slice (#1317). A multi-line comment (embedded `\n`) prints one
/// `// `-prefixed line per real line.
#[test]
fn prints_a_comment_one_prefixed_line_per_embedded_newline() {
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.\nSecond line.",
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "// Generated by bynk — do not edit by hand.\n// Second line.\n"
    );
}

/// Two adjacent top-level `Comment` statements get no blank line
/// between them — the same exception already established for two
/// adjacent `import`s, matching `events_fanout.rs`'s own real two-line
/// header banner (built as two separate `Comment` statements, not one
/// with an embedded newline).
#[test]
fn no_blank_line_between_two_adjacent_comment_statements() {
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment("Second line.", None));
    program.push(TsStmt::decl(
        TsDecl::Interface {
            name: "A".to_string(),
            type_params: Vec::new(),
            members: vec![],
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "// Generated by bynk — do not edit by hand.\n// Second line.\n\ninterface A {\n}\n"
    );
}

/// `TsExpr::multiline_object`, added for Arc C's own first real
/// conversion slice (#1317): `events_fanout.rs`'s own `__eventRoutes`
/// table is a top-level `const` initialiser with one entry per line,
/// each with its own trailing comma (including the last), closing
/// brace back at the statement's own indent — TypeScript's ordinary
/// multi-line object-literal convention, which `TsExpr::object`'s
/// single-line form cannot represent. Only reachable through a
/// statement/declaration-level renderer that carries `depth`
/// (`render_stmt_level_expr`) — this exercises it via a top-level
/// `ConstDecl`, the real shape `events_fanout.rs` itself uses.
#[test]
fn prints_a_multiline_object_as_a_top_level_const_initialiser() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ConstDecl {
            name: "table".to_string(),
            ty: None,
            init: TsExpr::multiline_object(vec![
                ("a".to_string(), TsExpr::Lit(TsLit::Num("1".to_string()))),
                ("b".to_string(), TsExpr::Lit(TsLit::Num("2".to_string()))),
            ]),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "const table = {\n  a: 1,\n  b: 2,\n};\n");
}

/// Review of #1317/#1318, finding 1: an empty `multiline_object` still
/// prints the open/close-brace-on-separate-lines shape, not the tight
/// `{}` a single-line empty object uses — matching the pre-conversion
/// `writeln!` code's own real behaviour (its `for` loop simply not
/// iterating, while the open-brace and closing `};` lines were written
/// unconditionally either way). This is a real, reachable shape:
/// `events_fanout.rs`'s own `__eventRoutes` table can be empty for a
/// context that publishes only events nobody subscribes to.
#[test]
fn an_empty_multiline_object_still_prints_the_open_and_close_brace_on_separate_lines() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ConstDecl {
            name: "table".to_string(),
            ty: None,
            init: TsExpr::multiline_object(vec![]),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "const table = {\n};\n");
}

/// A `multiline_object` reached through the ordinary, depth-unaware
/// `render_expr` recursion (nested inside another expression, not a
/// statement's own top-level initialiser) falls back to single-line
/// rendering — documented on `TsExpr::Object` itself as a real,
/// deliberate boundary, not silently wrong. Pinned here so a future
/// change to that boundary is a visible, intentional decision.
#[test]
fn a_nested_multiline_object_falls_back_to_single_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::array(vec![TsExpr::multiline_object(vec![(
            "a".to_string(),
            TsExpr::Lit(TsLit::Num("1".to_string())),
        )])]),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "[{ a: 1 }];\n");
}

/// Coverage gap named in review of #1314, finding 4: the blank-line-
/// between-top-level-declarations policy was never tested for the
/// mixed `Verbatim` + real-`Decl` case — the case that actually keeps
/// every P7.6 fixture's zero-diff claim true today, since every P7.6
/// construction site is all-`Verbatim`. Pins both halves of the rule's
/// asymmetry: no blank line is added *after* a `Verbatim` statement
/// (P7.7's own boundary — its content's own trailing spacing isn't the
/// printer's decision), but a blank line *is* added after a real
/// `Decl` even when `Verbatim` follows it.
#[test]
fn no_blank_line_after_verbatim_but_one_before_it() {
    let mut program = TsProgram::new();
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Contracts,
        "const legacy = 1;\n",
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::Interface {
            name: "A".to_string(),
            type_params: Vec::new(),
            members: vec![],
        },
        None,
    ));
    program.push(TsStmt::verbatim(
        VerbatimOrigin::Secrets,
        "const trailing = 2;\n",
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "const legacy = 1;\ninterface A {\n}\n\nconst trailing = 2;\n"
    );
}

/// Coverage gap named in review of #1314, finding 4: nothing exercised
/// `render_block_body`'s own graceful-degradation path — its doc
/// comment promises a single non-`Block` statement still renders
/// sensibly as a one-statement body rather than panicking, but no test
/// ever passed one. `TryCatch` is the real caller that could hit this
/// (its `try_block`/`catch_block` are typed as a bare `TsStmt`, not
/// required to be a `Block`).
#[test]
fn a_try_block_that_is_not_a_block_statement_still_prints_as_a_braced_body() {
    let mut program = TsProgram::new();
    program.push(TsStmt::try_catch(
        TsStmt::expr_stmt(
            TsExpr::Call {
                callee: Box::new(TsExpr::Ident("risky".to_string())),
                args: vec![],
            },
            None,
        ),
        Some("e"),
        TsStmt::expr_stmt(
            TsExpr::Call {
                callee: Box::new(TsExpr::Ident("handle".to_string())),
                args: vec![TsExpr::Ident("e".to_string())],
            },
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "try {\n  risky();\n} catch (e) {\n  handle(e);\n}\n"
    );
}

/// Coverage gap named in review of #1314, finding 4: `render_inline_stmt`'s
/// shared fallback arm (finding 3's fix — explicit variants, not a
/// wildcard) was never actually exercised by a test; every existing
/// brace-free `if`/`for...of` body used `Continue`/`Return`/`ExprStmt`,
/// which each have their own dedicated inline arm.
#[test]
fn a_brace_free_if_body_that_is_not_one_of_the_dedicated_inline_kinds_still_prints() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_stmt(
        TsExpr::Ident("cond".to_string()),
        TsStmt::const_stmt(
            TsBindingName::Ident("x".to_string()),
            None,
            TsExpr::Lit(TsLit::Num("1".to_string())),
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "if (cond) const x = 1;\n");
}

/// Review of #1314, finding 2: nothing checked operator-precedence
/// parenthesisation outside `As` — a `Binary` nested as the operand of
/// `Unary`/`Member`/`Await` printed with no parens at all, silently
/// changing what the text parses back to. Three of the review's own
/// examples, pinned directly.
#[test]
fn parenthesises_a_binary_operand_of_unary_member_and_await() {
    let bin = || TsExpr::Binary {
        op: TsBinaryOp::NullishCoalescing,
        left: Box::new(TsExpr::Ident("a".to_string())),
        right: Box::new(TsExpr::Ident("b".to_string())),
    };

    let mut not_program = TsProgram::new();
    not_program.push(TsStmt::expr_stmt(
        TsExpr::Unary {
            op: TsUnaryOp::Not,
            expr: Box::new(bin()),
        },
        None,
    ));
    assert_eq!(
        print(&not_program, "x.bynk", "", "x.ts").text,
        "!(a ?? b);\n"
    );

    let mut member_program = TsProgram::new();
    member_program.push(TsStmt::expr_stmt(
        TsExpr::Member {
            object: Box::new(bin()),
            property: "c".to_string(),
        },
        None,
    ));
    assert_eq!(
        print(&member_program, "x.bynk", "", "x.ts").text,
        "(a ?? b).c;\n"
    );

    let mut await_program = TsProgram::new();
    await_program.push(TsStmt::expr_stmt(TsExpr::Await(Box::new(bin())), None));
    assert_eq!(
        print(&await_program, "x.bynk", "", "x.ts").text,
        "await (a ?? b);\n"
    );
}

/// `TsUnaryOp::Neg` (1A.3, #3696): `-x`, a bracketed binary operand, and a
/// negated negation as `-(-x)` rather than the decrement `--x`.
#[test]
fn negation_prints_with_brackets_where_needed() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let neg = |e: TsExpr| TsExpr::Unary {
        op: TsUnaryOp::Neg,
        expr: Box::new(e),
    };
    assert_eq!(print_expr(&neg(id("x"))), "-x");
    let diff = TsExpr::Binary {
        op: TsBinaryOp::Add,
        left: Box::new(id("a")),
        right: Box::new(id("b")),
    };
    assert_eq!(print_expr(&neg(diff)), "-(a + b)");
    assert_eq!(print_expr(&neg(neg(id("x")))), "-(-x)");
}

/// `TsExpr::TypeApp` (1A.20, #3721, Decision A): the instantiation expression
/// `f<A, B>`, under a call, a `new` and a member chain, with a type argument
/// that is itself generic or an array. A `new` that is a member's object keeps
/// its `()`, and a `new` callee that reaches a call through its members is
/// bracketed.
#[test]
fn type_app_prints_as_an_instantiation_expression() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let member = |o: TsExpr, p: &str| TsExpr::Member {
        object: Box::new(o),
        property: p.to_string(),
    };
    let app = |c: TsExpr, args: Vec<TsType>| TsExpr::TypeApp {
        callee: Box::new(c),
        type_args: args,
    };
    let call = |c: TsExpr, args: Vec<TsExpr>| TsExpr::Call {
        callee: Box::new(c),
        args,
    };
    let new = |c: TsExpr, args: Vec<TsExpr>| TsExpr::New {
        callee: Box::new(c),
        args,
    };
    let named = |n: &str| TsType::named(n);
    assert_eq!(
        print_expr(&call(app(id("f"), vec![named("number")]), vec![id("a")])),
        "f<number>(a)"
    );
    assert_eq!(
        print_expr(&call(
            app(
                member(member(id("deps"), "Cap"), "op"),
                vec![named("A"), named("U")]
            ),
            vec![id("x"), id("y")]
        )),
        "deps.Cap.op<A, U>(x, y)"
    );
    assert_eq!(
        print_expr(&new(
            app(
                member(id("globalThis"), "Map"),
                vec![named("string"), named("number")]
            ),
            vec![]
        )),
        "new globalThis.Map<string, number>()"
    );
    let nested = TsType::Named {
        name: "Option".to_string(),
        type_args: vec![TsType::Array {
            element: Box::new(named("number")),
            readonly: true,
        }],
    };
    assert_eq!(
        print_expr(&call(app(id("__bynkSetOf"), vec![nested]), vec![id("xs")])),
        "__bynkSetOf<Option<readonly number[]>>(xs)"
    );
    // A `new` that is a member's object keeps its `()`.
    assert_eq!(
        print_expr(&call(
            member(
                new(member(id("globalThis"), "TextEncoder"), vec![]),
                "encode"
            ),
            vec![id("s")]
        )),
        "new globalThis.TextEncoder().encode(s)"
    );
    // A `new` callee reaching a call through its members is bracketed.
    assert_eq!(
        print_expr(&new(member(call(id("f"), vec![]), "C"), vec![])),
        "new (f().C)()"
    );
    assert_eq!(
        print_expr(&new(
            app(member(call(id("f"), vec![]), "C"), vec![named("T")]),
            vec![]
        )),
        "new (f().C<T>)()"
    );
}

/// `TsExpr::TypeApp` with no type arguments would print `f<>`, a syntax error.
#[test]
#[should_panic(expected = "TypeApp needs at least one type argument")]
fn type_app_with_no_type_arguments_is_refused() {
    let app = TsExpr::TypeApp {
        callee: Box::new(TsExpr::Ident("f".to_string())),
        type_args: vec![],
    };
    let _ = print_expr(&TsExpr::Call {
        callee: Box::new(app),
        args: vec![],
    });
}

/// `TsExpr::TypeApp` under a `Call` and a `New` keeps its type arguments in
/// order, and a bare instantiation of one argument prints `f<T>`.
#[test]
fn type_app_prints_one_and_many_type_arguments() {
    let app = |args: Vec<TsType>| TsExpr::TypeApp {
        callee: Box::new(TsExpr::Ident("f".to_string())),
        type_args: args,
    };
    assert_eq!(print_expr(&app(vec![TsType::named("T")])), "f<T>");
    assert_eq!(
        print_expr(&app(vec![TsType::named("A"), TsType::named("B")])),
        "f<A, B>"
    );
}

/// `TsExpr::NonNull` (1A.23, #3724): a postfix `!`, bracketing an operand that binds looser
/// than a postfix, as a member's object does.
#[test]
fn a_non_null_assertion_is_a_postfix() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let non_null = |e: TsExpr| TsExpr::NonNull(Box::new(e));
    assert_eq!(print_expr(&non_null(id("c"))), "c!");
    let member = TsExpr::Member {
        object: Box::new(id("a")),
        property: "b".to_string(),
    };
    assert_eq!(print_expr(&non_null(member)), "a.b!");
    let sum = TsExpr::Binary {
        op: TsBinaryOp::Add,
        left: Box::new(id("a")),
        right: Box::new(id("b")),
    };
    assert_eq!(print_expr(&non_null(sum)), "(a + b)!");
    let call = TsExpr::Call {
        callee: Box::new(non_null(id("f"))),
        args: vec![id("x")],
    };
    assert_eq!(print_expr(&call), "f!(x)");
}

/// A postfix `!` over an `await` or a unary operand is bracketed: `await x!` would assert
/// `x`, and `!x!` would negate `x!` (1A.23 review, #3779).
#[test]
fn a_non_null_assertion_brackets_await_and_unary_operands() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let awaited = TsExpr::Await(Box::new(id("x")));
    assert_eq!(
        print_expr(&TsExpr::NonNull(Box::new(awaited))),
        "(await x)!"
    );
    let negated = TsExpr::Unary {
        op: TsUnaryOp::Not,
        expr: Box::new(id("x")),
    };
    assert_eq!(print_expr(&TsExpr::NonNull(Box::new(negated))), "(!x)!");
}

/// `TsExpr::Spread` (1A.18a, #3719): `...x` as an array item, in any
/// position, over an identifier, a call, a bracketed operand and another
/// array; the operand prints as it would anywhere else, with no brackets of
/// the spread's own.
#[test]
fn spread_prints_as_an_array_item() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let spread = |e: TsExpr| TsExpr::Spread(Box::new(e));
    let arr = |items: Vec<TsExpr>| TsExpr::array(items);
    assert_eq!(
        print_expr(&arr(vec![id("a"), spread(id("b"))])),
        "[a, ...b]"
    );
    assert_eq!(
        print_expr(&arr(vec![spread(id("a")), id("b")])),
        "[...a, b]"
    );
    assert_eq!(
        print_expr(&arr(vec![spread(id("a")), spread(id("b"))])),
        "[...a, ...b]"
    );
    let call = TsExpr::Call {
        callee: Box::new(id("f")),
        args: vec![id("x")],
    };
    assert_eq!(print_expr(&arr(vec![spread(call)])), "[...f(x)]");
    let new_set = TsExpr::New {
        callee: Box::new(TsExpr::Member {
            object: Box::new(id("globalThis")),
            property: "Set".to_string(),
        }),
        args: vec![id("xs")],
    };
    assert_eq!(
        print_expr(&arr(vec![spread(new_set)])),
        "[...new globalThis.Set(xs)]"
    );
    let paren = TsExpr::Paren(Box::new(id("xs")));
    assert_eq!(print_expr(&arr(vec![spread(paren)])), "[...(xs)]");
    let inner = arr(vec![id("a")]);
    assert_eq!(print_expr(&arr(vec![spread(inner)])), "[...[a]]");
}

/// 1A.18b: a spread outside an array would print as `...a + b`, which reads
/// as a different tree. The printer fails loudly in a debug build rather
/// than misprint it (the same arm is the only way to reach it, since an
/// array item goes through its own renderer).
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "Spread is legal only as an array item or a call argument")]
fn a_spread_outside_an_array_is_refused() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let operand = TsExpr::Binary {
        op: TsBinaryOp::Add,
        left: Box::new(TsExpr::Spread(Box::new(id("a")))),
        right: Box::new(id("b")),
    };
    let _ = print_expr(&operand);
}

/// 1A.32c-1 review: only a `Prop` carries a doc; `with_doc` on any other
/// entry is refused in debug builds rather than silently dropped.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "with_doc: only a `Prop` entry carries a doc")]
fn a_doc_on_a_non_prop_entry_is_refused() {
    let _ = TsObjectEntry::Spread(TsExpr::Ident("rest".to_string())).with_doc("why");
}

/// 1A.18b: a spread is legal as an item of a multi-line array too.
#[test]
fn a_spread_prints_in_a_lines_array() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let lines = TsExpr::LinesArray(vec![TsExpr::Spread(Box::new(id("a"))), id("b")]);
    assert_eq!(print_expr(&lines), "[\n  ...a,\n  b,\n]");
}

/// `TsExpr::NullishAssign` (1A.36b-1): `target ??= value`, the accessor a cached
/// constant is built from, bracketed as an assignment is wherever it is an operand.
#[test]
fn a_nullish_assignment_prints_and_brackets() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let cached = TsExpr::NullishAssign {
        target: Box::new(id("__const_N")),
        value: Box::new(TsExpr::object(vec![("v".to_string(), id("n"))])),
    };
    assert_eq!(print_expr(&cached), "__const_N ??= { v: n }");
    let read = TsExpr::Member {
        object: Box::new(TsExpr::Paren(Box::new(cached.clone()))),
        property: "v".to_string(),
    };
    assert_eq!(print_expr(&read), "(__const_N ??= { v: n }).v");
    let operand = TsExpr::Binary {
        op: TsBinaryOp::Or,
        left: Box::new(cached),
        right: Box::new(id("x")),
    };
    assert_eq!(print_expr(&operand), "(__const_N ??= { v: n }) || x");
}

/// `TsExpr::RawUsing` (1A.36b-1): prints as its text, as an identifier does, and is
/// bracketed as one is.
#[test]
fn raw_using_prints_its_text_like_an_identifier() {
    let text = "(__j: __JsonValue) => { return Ok(__j); }";
    let raw = TsExpr::raw_using(text, vec!["Ok".to_string(), "__JsonValue".to_string()]);
    assert_eq!(print_expr(&raw), text);
    let call = TsExpr::Call {
        callee: Box::new(TsExpr::Ident("enc".to_string())),
        args: vec![raw.clone()],
    };
    assert_eq!(print_expr(&call), format!("enc({text})"));
    let member = TsExpr::Member {
        object: Box::new(raw),
        property: "x".to_string(),
    };
    assert_eq!(print_expr(&member), format!("{text}.x"));
}

/// `TsExpr::Assign` (1A.18b, #3719): an assignment expression, bracketed by
/// the builder where it is a member object (`(h[k] = h[k] ?? []).push(u)`),
/// and by the printer wherever it is an operand.
#[test]
fn an_assignment_expression_prints_and_brackets() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let slot = || TsExpr::Index {
        object: Box::new(id("h")),
        index: Box::new(id("k")),
    };
    let assign = TsExpr::Assign {
        target: Box::new(slot()),
        value: Box::new(TsExpr::Binary {
            op: TsBinaryOp::NullishCoalescing,
            left: Box::new(slot()),
            right: Box::new(TsExpr::array(vec![])),
        }),
    };
    assert_eq!(print_expr(&assign), "h[k] = h[k] ?? []");
    let push = TsExpr::Call {
        callee: Box::new(TsExpr::Member {
            object: Box::new(TsExpr::Paren(Box::new(assign.clone()))),
            property: "push".to_string(),
        }),
        args: vec![id("u")],
    };
    assert_eq!(print_expr(&push), "(h[k] = h[k] ?? []).push(u)");
    // Bare as an object, the printer brackets it.
    let bare = TsExpr::Member {
        object: Box::new(assign.clone()),
        property: "length".to_string(),
    };
    assert_eq!(print_expr(&bare), "(h[k] = h[k] ?? []).length");
    let sum = TsExpr::Binary {
        op: TsBinaryOp::Add,
        left: Box::new(id("a")),
        right: Box::new(assign),
    };
    assert_eq!(print_expr(&sum), "a + (h[k] = h[k] ?? [])");
}

#[test]
fn a_compound_assignment_prints_as_a_statement() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let add = TsExpr::AddAssign {
        target: Box::new(id("s")),
        value: Box::new(TsExpr::Call {
            callee: Box::new(id("f")),
            args: vec![id("x")],
        }),
    };
    assert_eq!(print_expr(&add), "s += f(x)");
    let stmt = TsStmt::expr_stmt(add, None);
    assert_eq!(print_stmt(&stmt, 0), "s += f(x);\n");
}

/// `TsType::Fn` with names (1A.18b, #3719): `(acc: A, x: T) => A`; without
/// them the positional `a0, a1` form is unchanged, and a short list names
/// the leading parameters and numbers the rest.
#[test]
fn a_function_type_prints_its_names_when_it_has_them() {
    let named = TsType::Fn {
        rest: false,
        params: vec![TsType::named("A"), TsType::named("T")],
        names: vec!["acc".to_string(), "x".to_string()],
        ret: Box::new(TsType::named("A")),
    };
    assert_eq!(print_type(&named), "(acc: A, x: T) => A");
    let positional = TsType::Fn {
        rest: false,
        params: vec![TsType::named("A"), TsType::named("T")],
        names: vec![],
        ret: Box::new(TsType::named("A")),
    };
    assert_eq!(print_type(&positional), "(a0: A, a1: T) => A");
    assert_eq!(
        print_type(&TsType::readonly_array(named)),
        "readonly ((acc: A, x: T) => A)[]"
    );
}

/// Review of #1314, finding 2's fourth example: a right-nested `Binary`
/// (`a ?? (b ?? c)`, as built) must not print as the bare `a ?? b ?? c`
/// that a naive left-to-right walk would produce — that text parses
/// back as `(a ?? b) ?? c`, a different tree than what was built.
#[test]
fn parenthesises_a_nested_binary_operand_of_another_binary() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::NullishCoalescing,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::NullishCoalescing,
                left: Box::new(TsExpr::Ident("b".to_string())),
                right: Box::new(TsExpr::Ident("c".to_string())),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a ?? (b ?? c);\n");
}

/// `??` is left-associative, so a left-nested chain prints flat and reparses as the tree it
/// was built from, while a right-nested one keeps its brackets (1B.5d: a driver's secret read
/// `(__bynkEnv["K"] ?? process.env["K"] ?? "")`).
#[test]
fn a_left_nested_nullish_chain_prints_flat() {
    let id = |s: &str| TsExpr::Ident(s.to_string());
    let nullish = |l: TsExpr, r: TsExpr| TsExpr::Binary {
        op: TsBinaryOp::NullishCoalescing,
        left: Box::new(l),
        right: Box::new(r),
    };
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        nullish(nullish(id("a"), id("b")), id("c")),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a ?? b ?? c;\n");
}

/// #2955: `*`/`**` sit above `+`, so a `**` right operand of `*` prints
/// flat (the `Decimal` range check's `c * 10n ** BigInt(s)`), a `+`
/// operand of `*` gets parens, and a same-operator `**` chain keeps them.
#[test]
fn multiplication_and_exponentiation_precedence() {
    let bin = |op, l: TsExpr, r: TsExpr| TsExpr::Binary {
        op,
        left: Box::new(l),
        right: Box::new(r),
    };
    let id = |s: &str| TsExpr::Ident(s.to_string());
    let cases = [
        (
            bin(
                TsBinaryOp::Mul,
                id("a"),
                bin(TsBinaryOp::Exp, id("b"), id("c")),
            ),
            "a * b ** c",
        ),
        (
            bin(
                TsBinaryOp::Mul,
                bin(TsBinaryOp::Add, id("a"), id("b")),
                id("c"),
            ),
            "(a + b) * c",
        ),
        (
            bin(
                TsBinaryOp::Exp,
                id("a"),
                bin(TsBinaryOp::Exp, id("b"), id("c")),
            ),
            "a ** (b ** c)",
        ),
        (
            bin(
                TsBinaryOp::GreaterThanEq,
                bin(TsBinaryOp::Mul, id("a"), id("b")),
                bin(TsBinaryOp::Mul, id("c"), id("d")),
            ),
            "a * b >= c * d",
        ),
    ];
    for (expr, want) in cases {
        assert_eq!(crate::print_expr(&expr), want);
    }
}

/// Slice 1A.5 (#3703), Decision C: the precedence/associativity audit for
/// every operator Bynk's value operators lower to, against TypeScript's
/// table. `+`/`-` share a tier and `*`/`/` the tier above; all four are
/// left-associative (left nesting flat, right nesting bracketed);
/// relational binds tighter than equality; unary binds tighter than any
/// binary and brackets a binary operand.
#[test]
fn value_operator_precedence_and_associativity_audit() {
    let bin = |op, l: TsExpr, r: TsExpr| TsExpr::Binary {
        op,
        left: Box::new(l),
        right: Box::new(r),
    };
    let un = |op, e: TsExpr| TsExpr::Unary {
        op,
        expr: Box::new(e),
    };
    let id = |s: &str| TsExpr::Ident(s.to_string());
    use TsBinaryOp::*;
    let cases = [
        (bin(Sub, bin(Sub, id("a"), id("b")), id("c")), "a - b - c"),
        (bin(Sub, id("a"), bin(Sub, id("b"), id("c"))), "a - (b - c)"),
        (bin(Add, id("a"), bin(Add, id("b"), id("c"))), "a + (b + c)"),
        (bin(Add, bin(Add, id("a"), id("b")), id("c")), "a + b + c"),
        (bin(Add, bin(Sub, id("a"), id("b")), id("c")), "a - b + c"),
        (bin(Add, id("a"), bin(Sub, id("b"), id("c"))), "a + (b - c)"),
        (bin(Sub, id("a"), bin(Mul, id("b"), id("c"))), "a - b * c"),
        (bin(Mul, bin(Add, id("a"), id("b")), id("c")), "(a + b) * c"),
        (bin(Mul, id("a"), bin(Add, id("b"), id("c"))), "a * (b + c)"),
        (bin(Mul, bin(Div, id("a"), id("b")), id("c")), "a / b * c"),
        (bin(Div, bin(Mul, id("a"), id("b")), id("c")), "a * b / c"),
        (bin(Div, id("a"), bin(Mul, id("b"), id("c"))), "a / (b * c)"),
        (bin(Div, id("a"), bin(Div, id("b"), id("c"))), "a / (b / c)"),
        (bin(Mul, bin(Mul, id("a"), id("b")), id("c")), "a * b * c"),
        (
            bin(
                StrictEq,
                bin(LessThan, id("a"), id("b")),
                bin(LessThan, id("c"), id("d")),
            ),
            "a < b === c < d",
        ),
        (
            bin(LessThan, bin(StrictEq, id("a"), id("b")), id("c")),
            "(a === b) < c",
        ),
        (
            bin(Add, id("a"), bin(LessThan, id("b"), id("c"))),
            "a + (b < c)",
        ),
        (
            un(TsUnaryOp::Not, bin(StrictEq, id("a"), id("b"))),
            "!(a === b)",
        ),
        (un(TsUnaryOp::Neg, bin(Mul, id("a"), id("b"))), "-(a * b)"),
        (bin(Mul, un(TsUnaryOp::Neg, id("a")), id("b")), "-a * b"),
        (bin(Sub, id("a"), un(TsUnaryOp::Neg, id("b"))), "a - -b"),
    ];
    for (expr, want) in cases {
        assert_eq!(crate::print_expr(&expr), want);
    }
}

/// #2955 review: `!a ** b` and `await a ** b` are JS syntax errors, so a
/// unary or `await` left operand of `**` is parenthesized; on the right
/// it needs none.
#[test]
fn exponentiation_parenthesizes_a_unary_left_operand() {
    let not_a = TsExpr::Unary {
        op: TsUnaryOp::Not,
        expr: Box::new(TsExpr::Ident("a".to_string())),
    };
    let exp = |l: TsExpr, r: TsExpr| TsExpr::Binary {
        op: TsBinaryOp::Exp,
        left: Box::new(l),
        right: Box::new(r),
    };
    let b = || TsExpr::Ident("b".to_string());
    assert_eq!(crate::print_expr(&exp(not_a.clone(), b())), "(!a) ** b");
    assert_eq!(crate::print_expr(&exp(b(), not_a)), "b ** !a");
    let await_a = TsExpr::Await(Box::new(TsExpr::Ident("a".to_string())));
    assert_eq!(crate::print_expr(&exp(await_a, b())), "(await a) ** b");
}

/// Review of #1314, finding 1: `TsLit::Str` escaped only `"`/`\`, so a
/// literal containing a newline, tab, or carriage return printed raw —
/// unterminated-string-literal TypeScript `tsc` can't parse. Pins the
/// fix against every character `crate::predicate::
/// escape_ts_literal` escapes.
#[test]
fn escapes_newline_tab_and_carriage_return_in_a_string_literal() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Lit(TsLit::Str("line one\nline two\ttabbed\r".to_string())),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "\"line one\\nline two\\ttabbed\\r\";\n");
}

// -- Arc C slice 3 (#1321, `workers.rs`): the five new shapes Decision A
// names, plus the real gaps found beyond it (`TsDecl::Function`/
// `TsDecl::TypeAlias`/`TsDecl::ImportNamespace`, the parameterless
// `catch {}` form, and the two new `TsUnaryOp`/four new `TsBinaryOp`
// operators). --

#[test]
fn prints_a_shorthand_async_method_entry_in_a_multiline_object() {
    let mut program = TsProgram::new();
    program.push(TsStmt::return_stmt(
        Some(TsExpr::multiline_object_entries(vec![
            TsObjectEntry::Method {
                name: "foo".to_string(),
                is_async: true,
                generics: Vec::new(),
                params: vec![TsParam::new("a".to_string(), Some(TsType::named("string")))],
                return_type: None,
                doc: None,
                inline: false,
                body: vec![TsStmt::return_stmt(
                    Some(TsExpr::Ident("a".to_string())),
                    None,
                )],
                span: None,
            },
        ])),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "return {\n  async foo(a: string) {\n    return a;\n  },\n};\n"
    );
}

/// `(a) => { <stmts> }` with a braced `if` first, a `const` and a `return`: the shape of the ICU
/// `select` arm. A body none of the compact forms holds on one line.
fn flat_probe_body() -> Vec<TsStmt> {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    vec![
        TsStmt::if_stmt(
            id("c"),
            TsStmt::block(
                vec![TsStmt::return_stmt(
                    Some(TsExpr::Lit(TsLit::Str("x".to_string()))),
                    None,
                )],
                None,
            ),
            None,
        ),
        TsStmt::const_stmt(
            TsBindingName::Ident("t".to_string()),
            Some(TsType::named("string")),
            id("a"),
            None,
        ),
        TsStmt::return_stmt(Some(id("t")), None),
    ]
}

fn arrow_over(body: TsArrowBody) -> TsExpr {
    TsExpr::Arrow {
        params: vec![TsParam::new("a".to_string(), None)],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(body),
    }
}

/// 1A.39a (Decision E): `TsArrowBody::Flat` prints each statement at depth 0 with its lines
/// trimmed and joined by single spaces, so a braced `if` sits on the one line.
#[test]
fn a_flat_body_joins_every_statement_onto_one_line() {
    assert_eq!(
        print_expr(&arrow_over(TsArrowBody::Flat(flat_probe_body()))),
        "(a) => { if (c) { return \"x\"; } const t: string = a; return t; }"
    );
    assert_eq!(
        print_expr(&arrow_over(TsArrowBody::Flat(Vec::new()))),
        "(a) => {}"
    );
}

/// 1A.39b: a single-line template literal flattens as any other text; a multi-line one is the
/// hazard the debug assertion names (flattening would trim the string's own whitespace).
#[test]
fn a_flat_body_allows_a_one_line_template_and_flags_a_multi_line_one() {
    assert!(!has_multiline_template("return `a ${b} c`;"));
    assert!(!has_multiline_template("if (c) {\n  return `x`;\n}"));
    assert!(has_multiline_template("return `a\n  b`;"));
    // A backtick inside a string literal is text: an odd one must not open a template.
    assert!(!has_multiline_template(
        "if (m) {\n  return \"press `Enter\";\n}"
    ));
    assert!(!has_multiline_template("if (m) {\n  return 'it`s';\n}"));
    assert!(!has_multiline_template("return \"a\\\"`\";\nx;"));
    let one_line = TsStmt::return_stmt(
        Some(TsExpr::template_lit(
            vec!["a".to_string(), "b".to_string()],
            vec![TsExpr::Ident("x".to_string())],
        )),
        None,
    );
    assert_eq!(
        print_expr(&arrow_over(TsArrowBody::Flat(vec![one_line]))),
        "(a) => { return `a${x}b`; }"
    );
}

/// 1A.39b (review): a message string holding an odd backtick (an ICU `select` arm's text,
/// which `escape_ts_literal` leaves as is), followed by the multi-line braced `if`, is a valid
/// `Flat` body: the backtick is string text, not a template delimiter.
#[test]
fn a_flat_body_with_a_backtick_in_a_string_literal_is_valid() {
    let body = vec![TsStmt::if_stmt(
        TsExpr::Ident("missing".to_string()),
        TsStmt::block(
            vec![TsStmt::return_stmt(
                Some(TsExpr::Lit(TsLit::Str("press `Enter".to_string()))),
                None,
            )],
            None,
        ),
        None,
    )];
    assert_eq!(
        print_expr(&arrow_over(TsArrowBody::Flat(body))),
        "(a) => { if (missing) { return \"press `Enter\"; } }"
    );
}

/// 1A.39a (Decision E): a `Block` body whose statement spans lines prints as the multi-line
/// body, where it used to trip `render_compact_stmts`'s assertion. A `Block` whose statements
/// each fit on one line is unchanged.
#[test]
fn a_block_body_falls_back_to_multiline_when_a_statement_spans_lines() {
    assert_eq!(
        print_expr(&arrow_over(TsArrowBody::Block(flat_probe_body()))),
        print_expr(&arrow_over(TsArrowBody::Multiline(flat_probe_body())))
    );
    let one_line = vec![
        TsStmt::const_stmt(
            TsBindingName::Ident("t".to_string()),
            None,
            TsExpr::Ident("a".to_string()),
            None,
        ),
        TsStmt::return_stmt(Some(TsExpr::Ident("t".to_string())), None),
    ];
    assert_eq!(
        print_expr(&arrow_over(TsArrowBody::Block(one_line))),
        "(a) => { const t = a; return t; }"
    );
}

/// 1A.39a (Decision E): `TsDecl::Function { inline: true }` is one line when its body fits,
/// and the ordinary multi-line function when a statement spans lines.
#[test]
fn an_inline_function_falls_back_to_multiline_when_its_body_spans_lines() {
    let function = |body: Vec<TsStmt>, inline: bool| {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: None,
                body,
                is_async: false,
                inline,
            },
            None,
        ));
        print(&program, "", "", "").text
    };
    let short = vec![TsStmt::return_stmt(
        Some(TsExpr::Ident("a".to_string())),
        None,
    )];
    assert_eq!(function(short, true), "function f() { return a; }\n");
    assert_eq!(
        function(flat_probe_body(), true),
        function(flat_probe_body(), false)
    );
}

#[test]
fn prints_shorthand_and_spread_entries_inline() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::object_entries(vec![
            TsObjectEntry::Spread(TsExpr::Ident("deps".to_string())),
            TsObjectEntry::Shorthand("cap1".to_string()),
            TsObjectEntry::prop(
                "identity".to_string(),
                TsExpr::Ident("__caller".to_string()),
            ),
        ]),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "{ ...deps, cap1, identity: __caller };\n");
}

#[test]
fn prints_an_expression_bodied_arrow() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Arrow {
            params: vec![TsParam::new(
                "events".to_string(),
                Some(TsType::named_with_args(
                    "Array",
                    vec![TsType::named("Wire")],
                )),
            )],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Call {
                callee: Box::new(TsExpr::Ident("dispatch".to_string())),
                args: vec![TsExpr::Ident("events".to_string())],
            }))),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "(events: Array<Wire>) => dispatch(events);\n");
}

/// #1327's own real gap: `emit_composition_root`'s `__eventsDispatch`
/// closure is the first real `Arrow` site that's async.
#[test]
fn prints_an_async_arrow() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Arrow {
            params: vec![TsParam::new(
                "events".to_string(),
                Some(TsType::named_with_args(
                    "Array",
                    vec![TsType::named("Wire")],
                )),
            )],
            is_async: true,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Flat(vec![TsStmt::expr_stmt(
                TsExpr::Call {
                    callee: Box::new(TsExpr::Ident("dispatch".to_string())),
                    args: vec![TsExpr::Ident("events".to_string())],
                },
                None,
            )])),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "async (events: Array<Wire>) => { dispatch(events); };\n"
    );
}

/// #1435 (Arc E slice 1): [`TsArrowBody::Block`], the first real
/// statement-bodied arrow — pins the exact `serialisation.rs` `Float`
/// non-finite guard shape (`serialise_field_expr_wire`'s own
/// `WireRef::Base { base: Float, .. }` arm) directly against a hand-built
/// tree, the same way #1322's own three parenthesisation tests below pin
/// their own gap. Proves `render_arrow_body`'s `Block` arm reuses
/// `render_compact_stmts` (an `If` with a brace-free `Throw` then-branch,
/// followed by a `Return`) correctly: both statements land on the SAME
/// physical line as the arrow's own `{ ... }`, with no trailing newline
/// leaking out of the block into the enclosing call's own `;`.
#[test]
fn prints_a_block_bodied_arrow_iife() {
    let mut program = TsProgram::new();
    let guard = TsExpr::Call {
        callee: Box::new(TsExpr::Arrow {
            params: vec![TsParam::new("v".to_string(), Some(TsType::named("number")))],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Block(vec![
                TsStmt::if_stmt(
                    TsExpr::Unary {
                        op: TsUnaryOp::Not,
                        expr: Box::new(TsExpr::Call {
                            callee: Box::new(TsExpr::Member {
                                object: Box::new(TsExpr::Ident("Number".to_string())),
                                property: "isFinite".to_string(),
                            }),
                            args: vec![TsExpr::Ident("v".to_string())],
                        }),
                    },
                    TsStmt::throw_stmt(
                        TsExpr::New {
                            callee: Box::new(TsExpr::Ident("Error".to_string())),
                            args: vec![TsExpr::Lit(TsLit::Str(
                                "non-finite Float at boundary".to_string(),
                            ))],
                        },
                        None,
                    ),
                    None,
                ),
                TsStmt::return_stmt(
                    Some(TsExpr::As {
                        expr: Box::new(TsExpr::Ident("v".to_string())),
                        ty: TsType::named("JsonValue"),
                    }),
                    None,
                ),
            ])),
        }),
        args: vec![TsExpr::Ident("value".to_string())],
    };
    program.push(TsStmt::expr_stmt(guard, None));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "((v: number) => { if (!Number.isFinite(v)) throw new Error(\"non-finite Float at boundary\"); return v as JsonValue; })(value);\n"
    );
}

/// Review of #1322, finding 1: `Arrow` was missing from every
/// parenthesisation rule. Not reachable through any real `bynk-emit`
/// content today (the one grounded `Arrow` site is an object-property
/// value, rendered through the depth-unaware `render_expr` with no
/// operand context) — these three tests pin the fix directly against
/// hand-built trees, the same way #1314's own precedence tests did
/// before any real content exercised them.
#[test]
fn parenthesises_an_arrow_used_as_a_call_callee() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Call {
            callee: Box::new(TsExpr::Arrow {
                params: vec![TsParam::new("x".to_string(), None)],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Ident("x".to_string())))),
            }),
            args: vec![TsExpr::Lit(TsLit::Num("1".to_string()))],
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "((x) => x)(1);\n");
}

#[test]
fn parenthesises_an_arrow_used_as_a_binary_operand() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::NullishCoalescing,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Arrow {
                params: vec![TsParam::new("x".to_string(), None)],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Ident("y".to_string())))),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a ?? ((x) => y);\n");
}

#[test]
fn parenthesises_an_arrow_used_as_an_as_operand() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::As {
            expr: Box::new(TsExpr::Arrow {
                params: vec![TsParam::new("x".to_string(), None)],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Ident("y".to_string())))),
            }),
            ty: TsType::named("Handler"),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "((x) => y) as Handler;\n");
}

#[test]
fn prints_optional_member_and_optional_index_chained() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::OptionalIndex {
            object: Box::new(TsExpr::OptionalMember {
                object: Box::new(TsExpr::Ident("x".to_string())),
                property: "process".to_string(),
            }),
            index: Box::new(TsExpr::Lit(TsLit::Str("secret".to_string()))),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "x?.process?.[\"secret\"];\n");
}

#[test]
fn print_type_renders_an_optional_object_field() {
    let ty = TsType::Object(vec![
        TsTypeMember::optional_prop("env", TsType::named("unknown")),
        TsTypeMember::prop("name", TsType::named("string")),
    ]);
    assert_eq!(print_type(&ty), "{ env?: unknown; name: string }");
}

#[test]
fn prints_a_top_level_function_declaration() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::Function {
            name: "compose".to_string(),
            generics: Vec::new(),
            params: vec![TsParam::new("env".to_string(), Some(TsType::named("Env")))],
            return_type: None,
            body: vec![TsStmt::return_stmt(Some(TsExpr::Lit(TsLit::Null)), None)],
            is_async: false,
            inline: false,
        })),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "export function compose(env: Env) {\n  return null;\n}\n"
    );
}

/// #1351's own real gap: `TsDecl::Function` had no `generics` field —
/// `emit_free_fn`'s own v0.20a erased generics (`export function
/// foo<A, B>(...)`) needed one. Review of #1352, finding 3: every other
/// existing `TsDecl::Function` test only threads `generics: Vec::new()`
/// through; this pins the non-empty case directly, matching #1339's own
/// precedent of a dedicated test per new field.
#[test]
fn prints_a_generic_top_level_function_declaration() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::Function {
            name: "identity".to_string(),
            generics: vec!["A".to_string(), "B".to_string()],
            params: vec![TsParam::new("x".to_string(), Some(TsType::named("A")))],
            return_type: Some(TsType::named("A")),
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::Ident("x".to_string())),
                None,
            )],
            is_async: false,
            inline: false,
        })),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "export function identity<A, B>(x: A): A {\n  return x;\n}\n"
    );
}

#[test]
fn prints_a_top_level_type_alias() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::TypeAlias {
            name: "Foo".to_string(),
            type_params: Vec::new(),
            ty: TsType::named("{ get(id: any): any }"),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "type Foo = { get(id: any): any };\n");
}

#[test]
fn prints_a_namespace_import_adjacent_to_a_named_import_with_no_blank_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Import {
            type_only: false,
            names: vec!["a".to_string()],
            from: "./x.js".to_string(),
        },
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::ImportNamespace {
            type_only: false,
            alias: "handlers".to_string(),
            from: "./handlers.js".to_string(),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "import { a } from \"./x.js\";\nimport * as handlers from \"./handlers.js\";\n"
    );
}

/// Arc C, step (10) (#1392): `ImportNamespace.type_only`, a parallel gap
/// by omission — `TsDecl::Import` already had this field, but nothing
/// before `emit_cross_context_namespace_imports`'s own conversion built
/// a type-only namespace import (`import type * as ns from "...";`, a
/// Workers-mode consumed-context import reaching the callee's types
/// only, #661).
#[test]
fn prints_a_type_only_namespace_import() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ImportNamespace {
            type_only: true,
            alias: "commerce_payment".to_string(),
            from: "../commerce-payment/handlers.js".to_string(),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "import type * as commerce_payment from \"../commerce-payment/handlers.js\";\n"
    );
}

/// Arc C, slice 37 (#1409, `tests_emit.rs`'s own slice G): `emit_integration_
/// module`'s own per-participant `import worker_{ns} from "../workers/{dir}/
/// index.js";` (the participant's Worker entry module's default export) —
/// the first real default import anywhere in `bynk-emit`'s own converted
/// content.
#[test]
fn prints_a_default_import() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ImportDefault {
            alias: "worker_shop_api".to_string(),
            named: Vec::new(),
            from: "../workers/shop-api/index.js".to_string(),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "import worker_shop_api from \"../workers/shop-api/index.js\";\n"
    );
}

/// #1321's own real gap beyond the accepted proposal's five: ES2019's
/// optional catch binding (`emit_http_sum_wrapper`'s own `} catch {`,
/// no `(e)` at all).
#[test]
fn prints_a_parameterless_catch() {
    let mut program = TsProgram::new();
    program.push(TsStmt::try_catch(
        TsStmt::block(vec![], None),
        None::<String>,
        TsStmt::block(vec![], None),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "try {\n} catch {\n}\n");
}

#[test]
fn prints_typeof_and_the_new_comparison_and_logical_operators() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Unary {
            op: TsUnaryOp::Typeof,
            expr: Box::new(TsExpr::Ident("x".to_string())),
        },
        None,
    ));
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::StrictEq,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Ident("b".to_string())),
        },
        None,
    ));
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::StrictNotEq,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Ident("b".to_string())),
        },
        None,
    ));
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::And,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Ident("b".to_string())),
        },
        None,
    ));
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::Or,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Ident("b".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "typeof x;\n\na === b;\n\na !== b;\n\na && b;\n\na || b;\n"
    );
}

/// #1321's own real gap: with only `??` in the algebra, a nested
/// `Binary` operand of another `Binary` was always parenthesized,
/// regardless of operator (see `parenthesises_a_nested_binary_operand_
/// of_another_binary`, unchanged). `workers.rs`'s own real content
/// nests a *strictly higher precedence* comparison inside `||`/`&&`
/// (`__authz === null || !__authz.startsWith(...)`) and the byte-golden
/// fixtures have no parens around it — this pins that the printer
/// omits them precisely in that case, not more broadly.
#[test]
fn a_strictly_higher_precedence_comparison_needs_no_parens_inside_or_or_and() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::Or,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Lit(TsLit::Null)),
            }),
            right: Box::new(TsExpr::Unary {
                op: TsUnaryOp::Not,
                expr: Box::new(TsExpr::Ident("b".to_string())),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a === null || !b;\n");
}

// -- #1323 (workers_entry.rs): Switch, ExportDefault, Conditional,
// TsType::Object readonly/method members, and the smaller mechanical
// gaps found during implementation (ReExport, Blank, TsLit::Bool,
// TsExpr::Paren, same-operator-chain flattening). --

/// #1323's own largest gap: `switch (<discriminant>) { <cases> }`. A
/// non-`default` `case` is `{ }`-blocked; `default` is not — pinned
/// against `workers_entry.rs`'s own real internal-dispatch shape.
#[test]
fn prints_a_switch_with_a_braced_case_and_an_unbraced_default() {
    let mut program = TsProgram::new();
    program.push(TsStmt::switch_stmt(
        TsExpr::Ident("servicePath".to_string()),
        vec![
            TsSwitchCase {
                labels: vec![TsExpr::Lit(TsLit::Str("orders".to_string()))],
                body: vec![TsStmt::return_stmt(None, None)],
                default_braced: false,
                case_braced: true,
                span: None,
            },
            TsSwitchCase {
                labels: vec![],
                body: vec![TsStmt::expr_stmt(
                    TsExpr::Call {
                        callee: Box::new(TsExpr::Member {
                            object: Box::new(TsExpr::Ident("console".to_string())),
                            property: "log".to_string(),
                        }),
                        args: vec![],
                    },
                    None,
                )],
                default_braced: false,
                case_braced: true,
                span: None,
            },
        ],
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "switch (servicePath) {\n  case \"orders\": {\n    return;\n  }\n  default:\n    console.log();\n}\n"
    );
}

/// Arc C, slice 33 (`tests_emit.rs` slice C, #1401): `TsSwitchCase.
/// default_braced` — `emit_stub_class`'s own `ReturnsEach` dispatch
/// braces its `default` case, unlike `workers_entry.rs`'s own unbraced
/// one pinned just above.
#[test]
fn prints_a_switch_with_a_braced_default() {
    let mut program = TsProgram::new();
    program.push(TsStmt::switch_stmt(
        TsExpr::Ident("__k".to_string()),
        vec![TsSwitchCase {
            labels: vec![],
            body: vec![TsStmt::expr_stmt(TsExpr::Ident("x".to_string()), None)],
            default_braced: true,
            case_braced: true,
            span: None,
        }],
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "switch (__k) {\n  default: {\n    x;\n  }\n}\n"
    );
}

/// Arc E slice 6 (#1445): `TsSwitchCase.case_braced` — the mirror-image
/// gap `default_braced` left open. `emit_sum_codec`'s own payload-free
/// variant case is unbraced right beside a braced payload-carrying
/// sibling in the *same* switch, pinned against `212_json_codec`'s own
/// mixed `Status` fixture (`case "Pending": return { kind: "Pending"
/// };` then `case "Shipped": { ... }`).
#[test]
fn prints_a_switch_with_an_unbraced_case_beside_a_braced_one() {
    let mut program = TsProgram::new();
    program.push(TsStmt::switch_stmt(
        TsExpr::Ident("kind".to_string()),
        vec![
            TsSwitchCase {
                labels: vec![TsExpr::Lit(TsLit::Str("Pending".to_string()))],
                body: vec![TsStmt::return_stmt(
                    Some(TsExpr::Ident("a".to_string())),
                    None,
                )],
                default_braced: false,
                case_braced: false,
                span: None,
            },
            TsSwitchCase {
                labels: vec![TsExpr::Lit(TsLit::Str("Shipped".to_string()))],
                body: vec![TsStmt::return_stmt(
                    Some(TsExpr::Ident("b".to_string())),
                    None,
                )],
                default_braced: false,
                case_braced: true,
                span: None,
            },
        ],
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "switch (kind) {\n  case \"Pending\":\n    return a;\n  case \"Shipped\": {\n    return b;\n  }\n}\n"
    );
}

/// #1323's own second gap: `export default <expr>;` — a default export
/// of a bare expression, not a declaration. Uses the same depth-aware
/// multiline-object handling `const`/`let`/`return` already get.
#[test]
fn prints_an_export_default_object_literal_multiline() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ExportDefault(TsExpr::multiline_object(vec![(
            "fetch".to_string(),
            TsExpr::Ident("handler".to_string()),
        )])),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "export default {\n  fetch: handler,\n};\n");
}

/// #1355's own real gap: a `multiline: true` `TsExpr::Object` nested as
/// one `Prop`'s own value inside ANOTHER multiline object —
/// `emit_messages_bundle`'s own real doubly-nested `{ locale: { code:
/// expr, ... }, ... }` table. Before this slice, `render_multiline_
/// object_entry`'s own `Prop` arm rendered its value through the plain,
/// depth-unaware `render_expr`, which ignores `multiline` entirely —
/// this would have silently collapsed the inner object to one line.
#[test]
fn prints_a_nested_multiline_object_as_a_props_own_value() {
    let mut program = TsProgram::new();
    program.push(TsStmt::const_stmt(
        TsBindingName::Ident("byLocale".to_string()),
        None,
        TsExpr::multiline_object_entries(vec![TsObjectEntry::prop(
            "\"en\"".to_string(),
            TsExpr::multiline_object_entries(vec![
                TsObjectEntry::prop(
                    "\"greeting\"".to_string(),
                    TsExpr::Ident("renderGreeting".to_string()),
                ),
                TsObjectEntry::prop(
                    "\"farewell\"".to_string(),
                    TsExpr::Ident("renderFarewell".to_string()),
                ),
            ]),
        )]),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "const byLocale = {\n  \"en\": {\n    \"greeting\": renderGreeting,\n    \"farewell\": renderFarewell,\n  },\n};\n"
    );
}

/// Review of #1356, finding 2: `render_multiline_object_entry`'s own
/// `Prop` arm routes through `render_stmt_level_expr` for BOTH
/// `TsExpr::Object`/`Array` (see `TsExpr::Array`'s own doc, updated by
/// #1355) — the object case is pinned above, this is the array sibling,
/// making that doc claim self-verifying rather than merely asserted.
#[test]
fn prints_a_nested_multiline_array_as_a_props_own_value() {
    let mut program = TsProgram::new();
    program.push(TsStmt::const_stmt(
        TsBindingName::Ident("byGroup".to_string()),
        None,
        TsExpr::multiline_object_entries(vec![TsObjectEntry::prop(
            "\"a\"".to_string(),
            TsExpr::multiline_array(vec![
                TsExpr::Ident("one".to_string()),
                TsExpr::Ident("two".to_string()),
            ]),
        )]),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "const byGroup = {\n  \"a\": [\n    one,\n    two,\n  ],\n};\n"
    );
}

/// #1323's own third gap: `test ? consequent : alternate`.
#[test]
fn prints_a_conditional_expression() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Conditional {
            test: Box::new(TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(TsExpr::Ident("method".to_string())),
                right: Box::new(TsExpr::Lit(TsLit::Str("HEAD".to_string()))),
            }),
            consequent: Box::new(TsExpr::Lit(TsLit::Num("1".to_string()))),
            alternate: Box::new(TsExpr::Lit(TsLit::Num("2".to_string()))),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "method === \"HEAD\" ? 1 : 2;\n");
}

/// Review-anticipated gap (matching #1322 finding 1's own class): a
/// `Conditional` used as a binary operand, an `as`-operand, or a call
/// callee all need parens — closed proactively in this same slice
/// rather than left for a review round to re-find.
#[test]
fn parenthesises_a_conditional_used_as_a_binary_operand_and_an_as_operand() {
    let cond = || TsExpr::Conditional {
        test: Box::new(TsExpr::Ident("a".to_string())),
        consequent: Box::new(TsExpr::Ident("b".to_string())),
        alternate: Box::new(TsExpr::Ident("c".to_string())),
    };
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::NullishCoalescing,
            left: Box::new(TsExpr::Ident("x".to_string())),
            right: Box::new(cond()),
        },
        None,
    ));
    program.push(TsStmt::expr_stmt(
        TsExpr::As {
            expr: Box::new(cond()),
            ty: TsType::named("string"),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "x ?? (a ? b : c);\n\n(a ? b : c) as string;\n"
    );
}

/// #1323's own real gap: a `Method` member of a type-position object
/// literal (no body), alongside a `readonly` property.
#[test]
fn print_type_renders_a_readonly_field_and_a_method_signature() {
    let ty = TsType::Object(vec![
        TsTypeMember::readonly_prop("cron", TsType::named("string")),
        TsTypeMember::method("ack", vec![], TsType::named("void")),
        TsTypeMember::method(
            "waitUntil",
            vec![TsParam::new(
                "promise".to_string(),
                Some(TsType::named_with_args(
                    "Promise",
                    vec![TsType::named("unknown")],
                )),
            )],
            TsType::named("void"),
        ),
    ]);
    assert_eq!(
        print_type(&ty),
        "{ readonly cron: string; ack(): void; waitUntil(promise: Promise<unknown>): void }"
    );
}

/// #1323's own real gap: `TsObjectEntry::Method` gained a
/// `return_type` field (mirroring `TsClassMethod`'s own existing one) —
/// `workers_entry.rs`'s own `export default { fetch, scheduled?,
/// queue? }` entries all carry an explicit return-type annotation,
/// unlike `workers.rs`'s own `compose`-returned wrapper methods
/// (`#1321`), none of which do.
#[test]
fn prints_a_multiline_object_methods_own_return_type_annotation() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ExportDefault(TsExpr::multiline_object_entries(vec![
            TsObjectEntry::Method {
                name: "fetch".to_string(),
                is_async: true,
                generics: Vec::new(),
                params: vec![],
                return_type: Some(TsType::named_with_args(
                    "Promise",
                    vec![TsType::named("Response")],
                )),
                doc: None,
                inline: false,
                body: vec![],
                span: None,
            },
        ])),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "export default {\n  async fetch(): Promise<Response> {\n  },\n};\n"
    );
}

/// #1323's own real correction: `Await` no longer auto-parenthesizes
/// under `As` — `workers_entry.rs`'s own real `await request.json() as
/// JsonValue` has no parens (`as` binds looser than `await`, so none
/// are grammatically needed; P7.8's original "always parenthesize"
/// reasoning conflated one file's own real text with a grammar
/// requirement).
#[test]
fn an_await_used_as_an_as_operand_no_longer_needs_parens() {
    let mut program = TsProgram::new();
    program.push(TsStmt::const_stmt(
        TsBindingName::Ident("args".to_string()),
        None,
        TsExpr::As {
            expr: Box::new(TsExpr::Await(Box::new(TsExpr::Call {
                callee: Box::new(TsExpr::Member {
                    object: Box::new(TsExpr::Ident("request".to_string())),
                    property: "json".to_string(),
                }),
                args: vec![],
            }))),
            ty: TsType::named("JsonValue"),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "const args = await request.json() as JsonValue;\n"
    );
}

/// #1353's own real gap: `bynk_ts::TsStmtKind::Throw`, added for
/// `emit_contract_guarded_body`'s own real `throw __e;` sites. Review of
/// #1354, finding 1: every real call site today reaches this only
/// through an `InlineBlock` (`render_stmt(out, stmt, 0)`), so the
/// top-level, non-inline `render_stmt` arm's own `indent(depth)` at a
/// nonzero depth was untested — pinned directly here, mirroring
/// `Return`'s own shape exactly.
#[test]
fn prints_a_throw_statement_at_a_nested_depth() {
    let stmt = TsStmt::throw_stmt(TsExpr::Ident("__e".to_string()), None);
    assert_eq!(print_stmt(&stmt, 2), "    throw __e;\n");
}

/// #1353's own real gap, byte-for-byte: the whole contract-guard shape
/// `emit_contract_guarded_body` builds (`if (!(pred)) { const __e = ...;
/// __e.name = ...; throw __e; }`) — an `If` over an `InlineBlock` of a
/// `Const`/`Assign`/`Throw` trio — asserted as one exact string. Review
/// of #1354, finding 1: no existing test (fixture, `contract_behaviour.
/// rs`, `source_map.rs`) asserts this shape's own bytes; they only
/// `contains`-check a runtime message or a generated line number, none
/// would catch a missing space, a reordered statement, or a lost
/// indent.
#[test]
fn prints_a_contract_guard_if_stmt() {
    let cond = TsExpr::Unary {
        op: TsUnaryOp::Not,
        expr: Box::new(TsExpr::Paren(Box::new(TsExpr::Binary {
            op: TsBinaryOp::GreaterThan,
            left: Box::new(TsExpr::Ident("n".to_string())),
            right: Box::new(TsExpr::Lit(TsLit::Num("0".to_string()))),
        }))),
    };
    let new_error = TsExpr::New {
        callee: Box::new(TsExpr::Ident("Error".to_string())),
        args: vec![TsExpr::template_lit(
            vec!["contract violated: precondition \\`positive\\` of scale (n=${n})".to_string()],
            Vec::new(),
        )],
    };
    let const_e = TsStmt::const_stmt(
        TsBindingName::Ident("__e".to_string()),
        None,
        new_error,
        None,
    );
    let assign_name = TsStmt::assign(
        TsExpr::Member {
            object: Box::new(TsExpr::Ident("__e".to_string())),
            property: "name".to_string(),
        },
        TsExpr::Lit(TsLit::Str("BynkContractError".to_string())),
        None,
    );
    let throw_e = TsStmt::throw_stmt(TsExpr::Ident("__e".to_string()), None);
    let then_branch = TsStmt::inline_block(vec![const_e, assign_name, throw_e], None);
    let if_stmt = TsStmt::if_stmt(cond, then_branch, None);
    assert_eq!(
        print_stmt(&if_stmt, 1),
        "  if (!(n > 0)) { const __e = new Error(`contract violated: precondition \\`positive\\` of scale (n=${n})`); __e.name = \"BynkContractError\"; throw __e; }\n"
    );
}

/// #1323's own real gap: `if`'s own `else` branch, printed on a fresh
/// line at the `if`'s own indent, then following the same block-vs-
/// inline rule the `if` itself does — `workers_entry.rs`'s own real
/// queue-consumer ack/retry dispatch.
#[test]
fn prints_an_if_else_with_an_inline_then_and_an_inline_block_else() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_else_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::StrictEq,
            left: Box::new(TsExpr::Member {
                object: Box::new(TsExpr::Ident("result".to_string())),
                property: "tag".to_string(),
            }),
            right: Box::new(TsExpr::Lit(TsLit::Str("Ack".to_string()))),
        },
        TsStmt::expr_stmt(
            TsExpr::Call {
                callee: Box::new(TsExpr::Member {
                    object: Box::new(TsExpr::Ident("msg".to_string())),
                    property: "ack".to_string(),
                }),
                args: vec![],
            },
            None,
        ),
        TsStmt::inline_block(
            vec![TsStmt::expr_stmt(
                TsExpr::Call {
                    callee: Box::new(TsExpr::Member {
                        object: Box::new(TsExpr::Ident("msg".to_string())),
                        property: "retry".to_string(),
                    }),
                    args: vec![],
                },
                None,
            )],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "if (result.tag === \"Ack\") msg.ack();\nelse { msg.retry(); }\n"
    );
}

/// #1323's own real gap: a `TryCatch`'s own `catch` body can be an
/// `InlineBlock` too — the braces stay on their own lines (this shape's
/// usual convention), but the content packs onto one compact line.
/// `workers_entry.rs`'s own real queue-consumer catch clause.
#[test]
fn a_try_catchs_own_catch_block_can_be_an_inline_block() {
    let mut program = TsProgram::new();
    program.push(TsStmt::try_catch(
        TsStmt::block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("work".to_string()), None)],
            None,
        ),
        Some("e"),
        TsStmt::inline_block(
            vec![
                TsStmt::expr_stmt(
                    TsExpr::Call {
                        callee: Box::new(TsExpr::Member {
                            object: Box::new(TsExpr::Ident("console".to_string())),
                            property: "error".to_string(),
                        }),
                        args: vec![TsExpr::Ident("e".to_string())],
                    },
                    None,
                ),
                TsStmt::expr_stmt(
                    TsExpr::Call {
                        callee: Box::new(TsExpr::Member {
                            object: Box::new(TsExpr::Ident("msg".to_string())),
                            property: "retry".to_string(),
                        }),
                        args: vec![],
                    },
                    None,
                ),
            ],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "try {\n  work;\n} catch (e) {\n  console.error(e); msg.retry();\n}\n"
    );
}

/// `TsExpr::DefaultIife` (1A.21): `((name = default): ret => { body })()`, as a call operand
/// and with a default that is a call or an object literal.
#[test]
fn a_default_iife_prints_its_argument_as_the_parameter_default() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let iife = |default: TsExpr| TsExpr::DefaultIife {
        name: "__pv".to_string(),
        default: Box::new(default),
        return_type: TsType::named("string"),
        body: vec![TsStmt::return_stmt(Some(id("__pv")), None)],
    };
    assert_eq!(
        print_expr(&iife(id("p"))),
        "((__pv = p): string => {\n  return __pv;\n})()"
    );
    let object = TsExpr::object(vec![("a".to_string(), id("b"))]);
    assert!(print_expr(&iife(object)).starts_with("((__pv = { a: b }): string => {"));
    let member = TsExpr::Member {
        object: Box::new(iife(id("p"))),
        property: "length".to_string(),
    };
    assert!(print_expr(&member).ends_with("})().length"));
}

/// An `if` whose inline block holds a multi-line expression (a block-bodied arrow in a
/// call argument, 1A.21) cannot be one line, so it takes the braced form, while a block of
/// one-line statements keeps `{ a; }`.
#[test]
fn an_inline_block_with_a_multiline_statement_takes_braces() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let arrow = TsExpr::Arrow {
        params: vec![],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Multiline(vec![TsStmt::return_stmt(
            Some(id("x")),
            None,
        )])),
    };
    let throw = |e: TsExpr| {
        TsStmt::throw_stmt(
            TsExpr::Call {
                callee: Box::new(id("fail")),
                args: vec![e],
            },
            None,
        )
    };
    let text = |stmt: TsStmt| {
        let mut program = TsProgram::new();
        program.push(stmt);
        print(&program, "x.bynk", "", "x.ts").text
    };
    assert_eq!(
        text(TsStmt::if_stmt(
            id("c"),
            TsStmt::inline_block(vec![throw(id("m"))], None),
            None
        )),
        "if (c) { throw fail(m); }\n"
    );
    assert_eq!(
        text(TsStmt::if_stmt(
            id("c"),
            TsStmt::inline_block(vec![throw(TsExpr::Paren(Box::new(arrow)))], None),
            None
        )),
        "if (c) {\n  throw fail((() => {\n    return x;\n  }));\n}\n"
    );
}

/// `TsStmtKind::TryCatch` with an optional `catch` and a `finally` (1A.21, #3722,
/// Decision C; reused by 1A.32): `try`/`catch`, `try`/`finally` and
/// `try`/`catch`/`finally`, with and without a catch binding.
#[test]
fn a_try_prints_catch_finally_or_both() {
    let work = || {
        TsStmt::block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("work".to_string()), None)],
            None,
        )
    };
    let handle = || {
        TsStmt::block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("handle".to_string()), None)],
            None,
        )
    };
    let tidy = || {
        TsStmt::block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("tidy".to_string()), None)],
            None,
        )
    };
    let text = |stmt: TsStmt| {
        let mut program = TsProgram::new();
        program.push(stmt);
        print(&program, "x.bynk", "", "x.ts").text
    };
    assert_eq!(
        text(TsStmt::try_catch(work(), Some("e"), handle(), None)),
        "try {\n  work;\n} catch (e) {\n  handle;\n}\n"
    );
    assert_eq!(
        text(TsStmt::try_finally(work(), tidy(), None)),
        "try {\n  work;\n} finally {\n  tidy;\n}\n"
    );
    assert_eq!(
        text(TsStmt::try_catch_finally(
            work(),
            Some("e"),
            handle(),
            tidy(),
            None
        )),
        "try {\n  work;\n} catch (e) {\n  handle;\n} finally {\n  tidy;\n}\n"
    );
    assert_eq!(
        text(TsStmt::try_catch_finally(
            work(),
            None::<String>,
            handle(),
            tidy(),
            None
        )),
        "try {\n  work;\n} catch {\n  handle;\n} finally {\n  tidy;\n}\n"
    );
}

/// A `try` with neither a `catch` nor a `finally` is a syntax error, so the printer
/// refuses it.
#[test]
#[should_panic(expected = "needs a `catch` or a `finally`")]
fn a_bare_try_is_refused() {
    let mut program = TsProgram::new();
    program.push(TsStmt {
        kind: TsStmtKind::TryCatch {
            try_block: Box::new(TsStmt::block(vec![], None)),
            catch_param: None,
            catch_block: None,
            finally_block: None,
        },
        span: None,
        nested_map: None,
        no_blank_before: false,
        nested_map_source_id: 0,
    });
    let _ = print(&program, "x.bynk", "", "x.ts");
}

/// #1323's own real gap: `TsBinaryOp::GreaterThan` — the request-body
/// ceiling guard's own `Number(__contentLength) > <cap>`, the one real
/// site needing a relational (not equality) comparison.
#[test]
fn prints_a_greater_than_comparison() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::GreaterThan,
            left: Box::new(TsExpr::Call {
                callee: Box::new(TsExpr::Ident("Number".to_string())),
                args: vec![TsExpr::Ident("__contentLength".to_string())],
            }),
            right: Box::new(TsExpr::Lit(TsLit::Num("100".to_string()))),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "Number(__contentLength) > 100;\n");
}

/// #1323's own real gap: `TsTypeMember::Index` — `workers_entry.rs`'s
/// own `args as { [k: string]: JsonValue }`, textually distinct from
/// the semantically-equivalent `Record<string, JsonValue>`.
#[test]
fn print_type_renders_an_index_signature() {
    let ty = TsType::Object(vec![TsTypeMember::index(
        "k",
        TsType::named("string"),
        TsType::named("JsonValue"),
    )]);
    assert_eq!(print_type(&ty), "{ [k: string]: JsonValue }");
}

/// #1323's own real gap, found implementing `workers_entry.rs`:
/// `export { a, b } from "spec";` — a re-export, distinct from both
/// `Import` and `Export`. Not classified alongside imports for the
/// "no blank line between adjacent imports" exception.
#[test]
fn prints_a_re_export_with_the_ordinary_blank_line_rule() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ImportNamespace {
            type_only: false,
            alias: "handlers".to_string(),
            from: "./handlers.js".to_string(),
        },
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::ReExport {
            names: vec!["Orders".to_string()],
            from: "./handlers.js".to_string(),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "import * as handlers from \"./handlers.js\";\n\nexport { Orders } from \"./handlers.js\";\n"
    );
}

/// #1323's own real gap, found implementing `workers_entry.rs`: a bare
/// blank line usable at any nesting depth, distinct from `print()`'s
/// own top-level-only policy.
#[test]
fn a_blank_statement_prints_one_empty_line_inside_a_nested_block() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_stmt(
        TsExpr::Ident("cond".to_string()),
        TsStmt::block(
            vec![
                TsStmt::expr_stmt(TsExpr::Ident("a".to_string()), None),
                TsStmt::blank(None),
                TsStmt::expr_stmt(TsExpr::Ident("b".to_string()), None),
            ],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "if (cond) {\n  a;\n\n  b;\n}\n");
}

/// #1323's own real gap, found implementing `workers_entry.rs`:
/// `CorsPolicy.credentials`/`SecurityPolicy.nosniff` are real booleans.
#[test]
fn prints_a_boolean_literal() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::object(vec![
            ("credentials".to_string(), TsExpr::Lit(TsLit::Bool(true))),
            ("nosniff".to_string(), TsExpr::Lit(TsLit::Bool(false))),
        ]),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "{ credentials: true, nosniff: false };\n");
}

/// #1323's own real gap: an explicit `Paren` always prints its own
/// literal parens, even when the wrapped expression's own precedence
/// would not otherwise need any — `workers_entry.rs`'s CORS-preflight
/// guard wraps its path-match condition in `(...)` unconditionally,
/// even for a single equality check with nothing lower-precedence than
/// the outer `&&` inside it (which the ordinary precedence-derived
/// rules correctly do NOT parenthesize on their own).
#[test]
fn an_explicit_paren_always_prints_regardless_of_the_inner_expressions_own_precedence() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::And,
            left: Box::new(TsExpr::Ident("method".to_string())),
            right: Box::new(TsExpr::Paren(Box::new(TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(TsExpr::Ident("path".to_string())),
                right: Box::new(TsExpr::Lit(TsLit::Str("/foo".to_string()))),
            }))),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "method && (path === \"/foo\");\n");
}

/// #1323's own real gap: a 3-term `||` chain of the *same* operator
/// prints flat, matching `emit_call_handler_dispatch`'s own real
/// `typeof args !== "object" || args === null || Array.isArray(args)`
/// — the pre-#1323 "always parenthesize equal precedence" rule would
/// have wrongly added parens around the first two terms.
#[test]
fn a_three_term_or_chain_of_the_same_operator_prints_flat() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::Or,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::Or,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::StrictNotEq,
                    left: Box::new(TsExpr::Unary {
                        op: TsUnaryOp::Typeof,
                        expr: Box::new(TsExpr::Ident("args".to_string())),
                    }),
                    right: Box::new(TsExpr::Lit(TsLit::Str("object".to_string()))),
                }),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::StrictEq,
                    left: Box::new(TsExpr::Ident("args".to_string())),
                    right: Box::new(TsExpr::Lit(TsLit::Null)),
                }),
            }),
            right: Box::new(TsExpr::Call {
                callee: Box::new(TsExpr::Member {
                    object: Box::new(TsExpr::Ident("Array".to_string())),
                    property: "isArray".to_string(),
                }),
                args: vec![TsExpr::Ident("args".to_string())],
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "typeof args !== \"object\" || args === null || Array.isArray(args);\n"
    );
}

/// Arc C, step (11) (#1388): `TsBinaryOp::Add`'s own real, dominant real
/// site — a message template's own literal/placeholder segments,
/// left-folded via `.join(" + ")` — needs the identical flat-chain
/// treatment `||`/`&&` already established, joining the same
/// associativity exemption in `render_binary_operand`, not a
/// bespoke third rule.
#[test]
fn a_three_term_add_chain_of_the_same_operator_prints_flat() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::Add,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Lit(TsLit::Str("a".to_string()))),
                right: Box::new(TsExpr::Lit(TsLit::Str("b".to_string()))),
            }),
            right: Box::new(TsExpr::Lit(TsLit::Str("c".to_string()))),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "\"a\" + \"b\" + \"c\";\n");
}

/// Review of #1389, finding 1: unlike `||`/`&&` (semantically
/// associative — flattening either side is safe), `+` is only
/// grammatically left-associative — `1 + (2 + "3")` and `(1 + 2) + "3"`
/// disagree once a number joins the chain. A right-nested `Add` must
/// keep its parens, matching every other non-`Or`/`And` same-operator
/// chain's own treatment (`a_right_nested_strict_eq_chain_keeps_its_
/// parens`/`a_right_nested_greater_than_chain_keeps_its_parens`).
#[test]
fn a_right_nested_add_chain_keeps_its_parens() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::Add,
            left: Box::new(TsExpr::Lit(TsLit::Num("1".to_string()))),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Lit(TsLit::Num("2".to_string()))),
                right: Box::new(TsExpr::Lit(TsLit::Str("3".to_string()))),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "1 + (2 + \"3\");\n");
}

/// `Add`'s own new `binary_precedence` entry (higher than
/// `GreaterThan`, matching real JS/TS) — a nested `Add` under a
/// `GreaterThan` needs no parens (`+` binds tighter), the reverse
/// direction does.
#[test]
fn add_binds_tighter_than_greater_than_on_both_sides() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::GreaterThan,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            }),
            right: Box::new(TsExpr::Ident("c".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a + b > c;\n");
}

/// The reverse nesting: a `GreaterThan` operand under an outer `Add`
/// needs its parens (`+` binds tighter, so `Add`'s own precedence check
/// against a lower-precedence `GreaterThan` operand must still fire).
#[test]
fn greater_than_nested_under_add_keeps_its_parens() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::Add,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::GreaterThan,
                left: Box::new(TsExpr::Ident("b".to_string())),
                right: Box::new(TsExpr::Ident("c".to_string())),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a + (b > c);\n");
}

/// Review of #1324, finding 1: the same-operator flattening #1323 added
/// for `||`/`&&` was wrongly applied to every operator, including
/// non-associative ones — a right-nested `StrictEq` inside `StrictEq`
/// must keep its parens (`a === b === c` parses as `(a === b) === c`,
/// not the tree's real `a === (b === c)`).
#[test]
fn a_right_nested_strict_eq_chain_keeps_its_parens() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::StrictEq,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(TsExpr::Ident("b".to_string())),
                right: Box::new(TsExpr::Ident("c".to_string())),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a === (b === c);\n");
}

/// Same finding, for the new `GreaterThan` operator: `a > b > c` parses
/// as `(a > b) > c` (a boolean compared against `c`), not the tree's
/// real `a > (b > c)` — a right-nested chain must keep its parens.
#[test]
fn a_right_nested_greater_than_chain_keeps_its_parens() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::GreaterThan,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::GreaterThan,
                left: Box::new(TsExpr::Ident("b".to_string())),
                right: Box::new(TsExpr::Ident("c".to_string())),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a > (b > c);\n");
}

/// Review of #1324, finding 2: a `Blank` used as an `if`'s brace-free
/// body used to render as a bare newline, silently letting the very
/// next statement in the enclosing block become the `if`'s own body —
/// now it prints an honest empty statement (`;`) that can't swallow
/// anything after it.
#[test]
fn a_blank_if_body_prints_an_empty_statement_not_a_swallowing_newline() {
    // Nested inside a `Block`, not two top-level program statements —
    // `print`'s own blank-line-between-statements policy only applies
    // between top-level statements (see its own doc comment), so a
    // top-level pair would mask the swallowing bug this test exists to
    // catch: the real hazard is two statements sharing one physical
    // line with nothing separating them, exactly what `render_stmt`'s
    // `Block` arm produces with no blank-line insertion of its own.
    let mut program = TsProgram::new();
    program.push(TsStmt::block(
        vec![
            TsStmt::if_stmt(TsExpr::Ident("cond".to_string()), TsStmt::blank(None), None),
            TsStmt::expr_stmt(
                TsExpr::Call {
                    callee: Box::new(TsExpr::Ident("nextStatement".to_string())),
                    args: vec![],
                },
                None,
            ),
        ],
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "{\n  if (cond) ;\n  nextStatement();\n}\n");
}

/// A left-nested same-operator chain also keeps its parens for a
/// non-associative operator (unlike the `||`/`&&` case above) —
/// `(a === b) === c` reads the same as `a === b === c` would parse, so
/// this is really about `render_binary_operand` not silently dropping
/// parens it should keep, not about a left/right asymmetry.
#[test]
fn a_left_nested_strict_eq_chain_keeps_its_parens() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::StrictEq,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            }),
            right: Box::new(TsExpr::Ident("c".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "(a === b) === c;\n");
}

// -- #1325: emit_test_main's own new shapes ------------------------------

#[test]
fn prints_a_template_literal_with_substitutions() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::template_lit(
            vec![
                String::new(),
                " passed, ".to_string(),
                " failed.".to_string(),
            ],
            vec![
                TsExpr::Ident("passed".to_string()),
                TsExpr::Ident("failed".to_string()),
            ],
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "`${passed} passed, ${failed} failed.`;\n");
}

/// A template literal's own static parts print with no escaping applied
/// by the printer — see `TsExpr::TemplateLit`'s own doc for why: a
/// generic escaper would double the literal backslash of an
/// already-pre-formed JS unicode escape (`✓`), corrupting it. This
/// pins the real, grounded shape directly: the six ASCII characters
/// `✓` pass through unchanged, not doubled into `\\u2713`.
#[test]
fn a_template_literal_part_carrying_a_preformed_unicode_escape_is_not_reescaped() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::template_lit(
            vec!["  \\u2713 ".to_string(), String::new()],
            vec![TsExpr::Ident("r".to_string())],
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "`  \\u2713 ${r}`;\n");
}

#[test]
fn a_raw_literal_prints_exactly_as_given_with_no_escaping() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Lit(TsLit::Raw("\"integration \\u00b7 \"".to_string())),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "\"integration \\u00b7 \";\n");
}

#[test]
fn prints_a_multiline_array_literal() {
    let mut program = TsProgram::new();
    program.push(TsStmt::const_stmt(
        TsBindingName::Ident("modules".to_string()),
        None,
        TsExpr::multiline_array(vec![
            TsExpr::object(vec![(
                "name".to_string(),
                TsExpr::Lit(TsLit::Str("a".to_string())),
            )]),
            TsExpr::object(vec![(
                "name".to_string(),
                TsExpr::Lit(TsLit::Str("b".to_string())),
            )]),
        ]),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "const modules = [\n  { name: \"a\" },\n  { name: \"b\" },\n];\n"
    );
}

/// Same reachability boundary as `TsExpr::Object`'s own `multiline`
/// field (see its doc) — a `multiline: true` array nested inside another
/// expression falls back to single-line via the depth-unaware
/// `render_expr` recursion.
#[test]
fn a_nested_multiline_array_falls_back_to_single_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::array(vec![TsExpr::multiline_array(vec![TsExpr::Lit(
            TsLit::Num("1".to_string()),
        )])]),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "[[1]];\n");
}

#[test]
fn prints_a_declare_const_ambient_binding() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::DeclareConst {
            name: "process".to_string(),
            ty: TsType::Object(vec![TsTypeMember::prop("env", TsType::named("unknown"))]),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "declare const process: { env: unknown };\n");
}

#[test]
fn prints_an_async_top_level_function() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Function {
            name: "main".to_string(),
            generics: Vec::new(),
            params: vec![],
            return_type: None,
            body: vec![TsStmt::return_stmt(None, None)],
            is_async: true,
            inline: false,
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "async function main() {\n  return;\n}\n");
}

/// #1369 (Arc C, slice 20): `TsDecl::Function.inline` — the zero-factory
/// shape (`function name(): Ret { return <expr>; }` all on one line)
/// `emit_agent` needs, mirroring `TsObjectEntry::Method.inline`'s own
/// single-line-vs-multi-line precedent (#1337) at a different node kind.
#[test]
fn prints_an_inline_top_level_function() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Function {
            name: "zero".to_string(),
            generics: Vec::new(),
            params: vec![],
            return_type: Some(TsType::named("Foo")),
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::object(vec![(
                    "n".to_string(),
                    TsExpr::Lit(TsLit::Num("0".to_string())),
                )])),
                None,
            )],
            is_async: false,
            inline: true,
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "function zero(): Foo { return { n: 0 }; }\n");
}

#[test]
fn prints_a_postfix_increment_statement() {
    let mut program = TsProgram::new();
    program.push(TsStmt::increment(TsExpr::Ident("passed".to_string()), None));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "passed++;\n");
}

/// An `Increment` inside an `InlineBlock` renders correctly through
/// `render_inline_stmt`'s fallback — `emit_test_main`'s own real
/// `{ passed++; console.log(...); }` shape.
#[test]
fn a_postfix_increment_renders_correctly_inside_an_inline_block() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_stmt(
        TsExpr::Ident("cond".to_string()),
        TsStmt::inline_block(
            vec![
                TsStmt::increment(TsExpr::Ident("passed".to_string()), None),
                TsStmt::expr_stmt(TsExpr::Ident("next".to_string()), None),
            ],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "if (cond) { passed++; next; }\n");
}

/// `same_line_else` with a `Block` then-branch — `} else {` on one
/// physical line, distinct from the fresh-line default pinned elsewhere
/// in this module.
#[test]
fn same_line_else_puts_the_else_keyword_after_the_closing_brace() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_else_same_line_stmt(
        TsExpr::Ident("cond".to_string()),
        TsStmt::block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("a".to_string()), None)],
            None,
        ),
        TsStmt::block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("b".to_string()), None)],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "if (cond) {\n  a;\n} else {\n  b;\n}\n");
}

/// `same_line_else` with `InlineBlock` branches on both sides —
/// `emit_test_main`'s own real `if (r.pass) { ... } else { ... }` shape,
/// entirely on one generated line.
#[test]
fn same_line_else_with_inline_block_branches_stays_on_one_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_else_same_line_stmt(
        TsExpr::Ident("cond".to_string()),
        TsStmt::inline_block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("a".to_string()), None)],
            None,
        ),
        TsStmt::inline_block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("b".to_string()), None)],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "if (cond) { a; } else { b; }\n");
}

/// `same_line_else` with a brace-free `then_branch` falls back to the
/// ordinary fresh-line rendering — nothing real needs `<inline-stmt>
/// else {`, and the printer doesn't claim to support it (see
/// `TsStmtKind::If`'s own doc).
#[test]
fn same_line_else_with_a_brace_free_then_branch_falls_back_to_fresh_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::if_else_same_line_stmt(
        TsExpr::Ident("cond".to_string()),
        TsStmt::continue_stmt(None),
        TsStmt::block(
            vec![TsStmt::expr_stmt(TsExpr::Ident("b".to_string()), None)],
            None,
        ),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "if (cond) continue;\nelse {\n  b;\n}\n");
}

/// `TsDecl::ReExportAll` prints `export * from "spec";` — no braces, no
/// name list, matching `emit_commons_barrel`'s own real per-file line.
#[test]
fn re_export_all_prints_a_wildcard_re_export() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ReExportAll {
            from: "./thing/make.js".to_string(),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "export * from \"./thing/make.js\";\n");
}

/// Review of #1329's own grounding: `emit_commons_barrel`'s real barrel
/// module is one header `Comment` immediately followed by one `export
/// *` line per constituent source file, every one of those lines
/// adjacent with no blank line anywhere — the exact byte shape pinned
/// here, matching `251_multi_file_commons_test`'s own real
/// `expected/thing.ts`.
#[test]
fn a_header_comment_and_consecutive_re_export_alls_have_no_blank_lines() {
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::ReExportAll {
            from: "./thing/make.js".to_string(),
        },
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::ReExportAll {
            from: "./thing/widget.js".to_string(),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "// Generated by bynk — do not edit by hand.\n\
         export * from \"./thing/make.js\";\n\
         export * from \"./thing/widget.js\";\n"
    );
}

/// Review of #1330: the grouping rule added for #1329 is scoped to
/// `ReExportAll`-adjacent-to-`ReExportAll` (and a `Comment` immediately
/// before one) — a `ReExportAll` next to anything else still gets the
/// ordinary blank line. `emit_commons_barrel` is `ReExportAll`'s only
/// producer today, so this is the invariant keeping the rule scoped if
/// a future slice ever gives it a second one.
#[test]
fn a_re_export_all_next_to_a_non_re_export_all_gets_the_ordinary_blank_line() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ImportNamespace {
            type_only: false,
            alias: "handlers".to_string(),
            from: "./handlers.js".to_string(),
        },
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::ReExportAll {
            from: "./thing/make.js".to_string(),
        },
        None,
    ));
    program.push(TsStmt::comment("trailing note", None));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "import * as handlers from \"./handlers.js\";\n\n\
         export * from \"./thing/make.js\";\n\n\
         // trailing note\n"
    );
}

/// #1333: `print_stmt` prints a `TsStmtKind::DocComment` as a real JSDoc
/// block, matching `emit_doc_block`'s own real multi-line shape
/// (`137_agent_instantiation_workers/expected/workers/demo-counter/
/// handlers.ts`'s own real header comment).
#[test]
fn print_stmt_renders_a_multi_line_doc_comment() {
    let stmt = TsStmt::doc_comment(
        "A minimal stateful agent in the bundle target: instantiation lowers through the\ngenerated factory, the method call is a direct call, and state persists per key\nacross calls within a session.",
        None,
    );
    assert_eq!(
        print_stmt(&stmt, 0),
        "/**\n \
         * A minimal stateful agent in the bundle target: instantiation lowers through the\n \
         * generated factory, the method call is a direct call, and state persists per key\n \
         * across calls within a session.\n \
         */\n"
    );
}

/// #1333: a literal `*/` inside doc text escapes to `*\/`, matching
/// `emit_doc_block`'s own pre-conversion behaviour exactly (issue
/// #720 — an unescaped `*/` would otherwise close the comment early
/// and let trailing text land as executable top-level TypeScript).
#[test]
fn print_stmt_escapes_a_literal_comment_terminator_inside_doc_text() {
    let stmt = TsStmt::doc_comment("docs */ ; (globalThis as any).PWNED = true; /*", None);
    let printed = print_stmt(&stmt, 0);
    assert!(!printed.contains("*/ ;"), "unescaped terminator: {printed}");
    assert_eq!(
        printed,
        "/**\n * docs *\\/ ; (globalThis as any).PWNED = true; /*\n */\n"
    );
}

/// #1333: a blank line inside doc text prints as a bare ` *`, no
/// trailing space — distinct from a non-blank line's own ` * <line>`.
#[test]
fn print_stmt_renders_a_blank_doc_line_as_a_bare_star() {
    let stmt = TsStmt::doc_comment("first paragraph\n\nsecond paragraph", None);
    assert_eq!(
        print_stmt(&stmt, 0),
        "/**\n * first paragraph\n *\n * second paragraph\n */\n"
    );
}

/// #1333: `print_stmt`'s own `depth` parameter indents every line of
/// the JSDoc block, matching `render_stmt`'s own 2-space-per-level
/// convention — `emit_doc_block`'s real callers pass `INDENT_STEP`
/// (2 raw spaces = depth 1) for a nested doc comment.
#[test]
fn print_stmt_indents_a_doc_comment_at_depth() {
    let stmt = TsStmt::doc_comment("a method", None);
    assert_eq!(print_stmt(&stmt, 1), "  /**\n   * a method\n   */\n");
}

/// #1337: `TsStmtKind::Raw` prints its own text exactly as given — no
/// leading indent (unlike every other statement kind, whose own
/// `render_stmt` arm prefixes `indent(depth)`), no added semicolon or
/// braces. `emit_method`'s own opaque `lower.rs`-sourced body is
/// already fully, absolutely indented by the time it's captured into
/// one `Raw` node, so the printer must contribute nothing further —
/// the same reasoning `Verbatim` already established for pre-rendered
/// content, `Raw`'s own doc explains why it's a distinct kind.
#[test]
fn print_stmt_renders_raw_text_verbatim_with_no_added_indent_or_punctuation() {
    let stmt = TsStmt::raw("    return x + 1;\n", None);
    // depth is passed but deliberately has no effect on Raw's own output.
    assert_eq!(print_stmt(&stmt, 3), "    return x + 1;\n");
}

/// #1337: multi-line `Raw` text (the real shape — a whole function
/// body, not one line) passes through with every embedded line intact,
/// confirming the printer doesn't split, re-indent, or otherwise
/// interpret it.
#[test]
fn print_stmt_renders_multi_line_raw_text_unchanged() {
    let stmt = TsStmt::raw(
        "    const r = Uuid.of(crypto.randomUUID());\n    return r.value;\n",
        None,
    );
    assert_eq!(
        print_stmt(&stmt, 0),
        "    const r = Uuid.of(crypto.randomUUID());\n    return r.value;\n"
    );
}

/// #1337: `TsObjectEntry::Method` with a `Raw` body — `emit_method`'s
/// own real shape (`{method}{generics}({params}): {ret} { <raw body>
/// },`), pinned at the actual depth its own object literal renders at
/// (a top-level `export const {...}`, depth 0 → entries at depth 1,
/// two-space indent, matching `emit_method`'s own pre-conversion
/// hand-written `"  {method}..."` line).
#[test]
fn multiline_object_renders_a_method_entry_with_a_raw_body() {
    let mut out = String::new();
    render_multiline_object(
        &mut out,
        &[TsObjectEntry::Method {
            name: "of".to_string(),
            is_async: false,
            generics: Vec::new(),
            params: vec![TsParam::new(
                "value".to_string(),
                Some(TsType::named("string")),
            )],
            return_type: Some(TsType::named("Uuid")),
            doc: None,
            inline: false,
            body: vec![TsStmt::raw("    return value as Uuid;\n", None)],
            span: None,
        }],
        None,
        0,
        &mut None,
    );
    assert_eq!(
        out,
        "{\n  of(value: string): Uuid {\n    return value as Uuid;\n  },\n}"
    );
}

/// #1337: `TsObjectEntry::Method`'s own `generics` field prints
/// `<A, U>` between the method name and its parameter list —
/// `Box.map`'s own real shape (`402_generic_instance_method`, a
/// single-file-form fixture the accepted proposal's own project-form
/// search missed).
#[test]
fn multiline_object_renders_a_generic_method_entry() {
    let mut out = String::new();
    render_multiline_object(
        &mut out,
        &[TsObjectEntry::Method {
            name: "map".to_string(),
            is_async: false,
            generics: vec!["A".to_string(), "U".to_string()],
            params: vec![
                TsParam::new("self".to_string(), Some(TsType::named("Box<A>"))),
                TsParam::new("f".to_string(), Some(TsType::named("(a0: A) => U"))),
            ],
            return_type: Some(TsType::named("Box<U>")),
            doc: None,
            inline: false,
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::object(vec![(
                    "value".to_string(),
                    TsExpr::Call {
                        callee: Box::new(TsExpr::Ident("f".to_string())),
                        args: vec![TsExpr::Ident("self.value".to_string())],
                    },
                )])),
                None,
            )],
            span: None,
        }],
        None,
        0,
        &mut None,
    );
    assert_eq!(
        out,
        "{\n  map<A, U>(self: Box<A>, f: (a0: A) => U): Box<U> {\n    return { value: f(self.value) };\n  },\n}"
    );
}

/// #1337: a method with no generics prints no `<>` at all — the
/// ordinary, dominant case (every prior slice's own real method
/// entries), confirming `generics: Vec::new()` is a true no-op, not
/// an empty `<>`.
#[test]
fn multiline_object_renders_a_non_generic_method_entry_with_no_angle_brackets() {
    let mut out = String::new();
    render_multiline_object(
        &mut out,
        &[TsObjectEntry::Method {
            name: "get".to_string(),
            is_async: false,
            generics: Vec::new(),
            params: vec![],
            return_type: Some(TsType::named("void")),
            doc: None,
            inline: false,
            body: vec![],
            span: None,
        }],
        None,
        0,
        &mut None,
    );
    assert_eq!(out, "{\n  get(): void {\n  },\n}");
}

/// #1337: `TsObjectEntry::Method`'s own `doc` field prints a JSDoc
/// block immediately before the method entry, same indent, no blank
/// line between — `Timestamp.diff`'s own real shape
/// (`65_money_uses_time`), reusing `render_doc_comment` (the same
/// renderer `TsStmtKind::DocComment` already uses) rather than a
/// second copy.
#[test]
fn multiline_object_renders_a_method_entry_with_a_preceding_doc_comment() {
    let mut out = String::new();
    render_multiline_object(
        &mut out,
        &[TsObjectEntry::Method {
            name: "diff".to_string(),
            is_async: false,
            generics: Vec::new(),
            params: vec![TsParam::new(
                "self".to_string(),
                Some(TsType::named("Timestamp")),
            )],
            return_type: Some(TsType::named("Span")),
            doc: Some("Compute the duration between two timestamps.".to_string()),
            inline: false,
            body: vec![TsStmt::raw("    return 0;\n", None)],
            span: None,
        }],
        None,
        0,
        &mut None,
    );
    assert_eq!(
        out,
        "{\n  /**\n   * Compute the duration between two timestamps.\n   */\n  diff(self: Timestamp): Span {\n    return 0;\n  },\n}"
    );
}

/// #1337: `TsObjectEntry::Method`'s own `inline: true` renders the
/// whole entry — signature and one-statement body alike — on ONE
/// physical line, `emit_forwarded_methods`'s own real shape
/// (`255_context_uses_commons_static_method`'s own real `equals`
/// entry: `equals(self: Cents, other: Cents): boolean { return
/// __CommonsCents.equals(self, other) as unknown as boolean; },`),
/// distinct from every other real `Method` entry in this tree
/// (always multi-line, `inline: false`).
#[test]
fn multiline_object_renders_an_inline_method_entry_on_one_line() {
    let mut out = String::new();
    render_multiline_object(
        &mut out,
        &[TsObjectEntry::Method {
            name: "equals".to_string(),
            is_async: false,
            generics: Vec::new(),
            params: vec![
                TsParam::new("self".to_string(), Some(TsType::named("Cents"))),
                TsParam::new("other".to_string(), Some(TsType::named("Cents"))),
            ],
            return_type: Some(TsType::named("boolean")),
            doc: None,
            inline: true,
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::As {
                    expr: Box::new(TsExpr::As {
                        expr: Box::new(TsExpr::Call {
                            callee: Box::new(TsExpr::Member {
                                object: Box::new(TsExpr::Ident("__CommonsCents".to_string())),
                                property: "equals".to_string(),
                            }),
                            args: vec![
                                TsExpr::Ident("self".to_string()),
                                TsExpr::Ident("other".to_string()),
                            ],
                        }),
                        ty: TsType::named("unknown"),
                    }),
                    ty: TsType::named("boolean"),
                }),
                None,
            )],
            span: None,
        }],
        None,
        0,
        &mut None,
    );
    assert_eq!(
        out,
        "{\n  equals(self: Cents, other: Cents): boolean { return __CommonsCents.equals(self, other) as unknown as boolean; },\n}"
    );
}

/// Review of #1338, finding 4: `print_object_entry` — the exact public
/// API `emit_refined_type`/`emit_record_type`/`emit_sum_type`'s own
/// real call sites depend on — had no direct test; every #1337 test
/// above drives `render_multiline_object` instead. Pins the contract
/// those call sites rely on: `print_object_entry(&entry, 0)` produces
/// the same text as the entry-slice portion of what
/// `render_multiline_object`'s own depth-0 output produces for the
/// same entry (its own depth convention — the entry lands one level
/// deeper than the object, i.e. `depth + 1` — matches exactly, not
/// coincidentally, since both paths route through the same
/// `render_multiline_object_entry`).
#[test]
fn print_object_entry_matches_the_multiline_objects_own_entry_slice() {
    // A real `TsStmt::return_stmt` body, not `Raw` — `Raw`'s own baked-in
    // indent only matches depth 0 (finding 3's own new guard), so an
    // ordinary real-node body is what this depth-convention contract
    // needs to prove at a non-zero depth.
    let entry = TsObjectEntry::Method {
        name: "of".to_string(),
        is_async: false,
        generics: Vec::new(),
        params: vec![TsParam::new(
            "value".to_string(),
            Some(TsType::named("string")),
        )],
        return_type: Some(TsType::named("Uuid")),
        doc: None,
        inline: false,
        body: vec![TsStmt::return_stmt(
            Some(TsExpr::As {
                expr: Box::new(TsExpr::Ident("value".to_string())),
                ty: TsType::named("Uuid"),
            }),
            None,
        )],
        span: None,
    };

    let mut whole_object = String::new();
    render_multiline_object(
        &mut whole_object,
        std::slice::from_ref(&entry),
        None,
        0,
        &mut None,
    );
    // `render_multiline_object`'s own entry line is followed by `\n`
    // then the closing `}` (no blank line between them for a single
    // entry) — `print_object_entry`'s own output keeps that same
    // trailing `\n` (it has no closing brace of its own to attach to),
    // so the real equivalence is entry-slice-plus-newline, not a bare
    // slice.
    let entry_slice_with_trailing_newline = whole_object
        .strip_prefix("{\n")
        .and_then(|s| s.strip_suffix('}'))
        .expect("render_multiline_object's own single-entry output has a { }-wrapper");

    assert_eq!(
        print_object_entry(&entry, 0),
        entry_slice_with_trailing_newline
    );
}

/// #1339's own real gap: `emit_refined_type`'s own branded-type alias,
/// `{base} & { readonly __brand: "..." }`, has no representation among
/// `Named`/`Array`/`Object`/`Fn`/`Union` — mirrors `Union`'s own single-
/// line, ` & `-joined shape exactly.
#[test]
fn print_type_renders_an_intersection() {
    let ty = TsType::intersection(vec![
        TsType::named("string"),
        TsType::Object(vec![TsTypeMember::readonly_prop(
            "__brand",
            TsType::named("\"Order\""),
        )]),
    ]);
    assert_eq!(print_type(&ty), "string & { readonly __brand: \"Order\" }");
}

/// #1339's own real gap: `emit_sum_type`'s own multi-line discriminated
/// union — a leading `|` on every line except the first (which gets
/// equivalent spacing instead, matching the pre-conversion `writeln!`
/// code's own `let pipe = if i == 0 { " " } else { "|" };` exactly), no
/// trailing newline or `;` of its own (the caller, `TsDecl::TypeAlias`'s
/// own render arm, owns both).
#[test]
fn print_type_renders_a_multiline_union() {
    let ty = TsType::multiline_union(vec![
        TsType::Object(vec![TsTypeMember::readonly_prop(
            "tag",
            TsType::named("\"a\""),
        )]),
        TsType::Object(vec![
            TsTypeMember::readonly_prop("tag", TsType::named("\"b\"")),
            TsTypeMember::readonly_prop("value", TsType::named("number")),
        ]),
    ]);
    assert_eq!(
        print_type(&ty),
        "    { readonly tag: \"a\" }\n  | { readonly tag: \"b\"; readonly value: number }"
    );
}

/// #1339: `TsDecl::TypeAlias`'s own multiline-union special case — the
/// `=` is followed directly by `\n` (no trailing space, matching the
/// pre-conversion `writeln!(out, "export type {name}{params} =")`
/// line's own exact bytes), each variant on its own line, the closing
/// `;` appended directly to the last variant's own line. Also pins
/// `type_params`' own bare-generics rendering on the alias header.
#[test]
fn prints_a_generic_sum_types_own_multiline_type_alias() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::TypeAlias {
            name: "Opt".to_string(),
            type_params: vec!["T".to_string()],
            ty: TsType::multiline_union(vec![
                TsType::Object(vec![TsTypeMember::readonly_prop(
                    "tag",
                    TsType::named("\"none\""),
                )]),
                TsType::Object(vec![
                    TsTypeMember::readonly_prop("tag", TsType::named("\"some\"")),
                    TsTypeMember::readonly_prop("value", TsType::named("T")),
                ]),
            ]),
        })),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "export type Opt<T> =\n    \
         { readonly tag: \"none\" }\n  \
         | { readonly tag: \"some\"; readonly value: T };\n"
    );
}

/// #1339's own real gap: `emit_record_type`'s own `export interface
/// {name}{params} { readonly {field}: {ty}; ... }` — bare generic names
/// on the interface header, `readonly` on every real member here.
#[test]
fn prints_a_generic_interface_with_readonly_members() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::Interface {
            name: "Box".to_string(),
            type_params: vec!["T".to_string()],
            members: vec![TsTypeMember::readonly_prop("value", TsType::named("T"))],
        })),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "export interface Box<T> {\n  readonly value: T;\n}\n"
    );
}

/// #1357's own real gap: `TsTypeMember::Method` had no `generics`/`doc`
/// fields — `emit_capability`'s own interface methods are genuinely
/// generic (no monomorphisation) and doc-commented per op. `doc` renders
/// at `TsDecl::Interface`'s own render arm (not `render_type_member`
/// itself, which has no `depth` to give `render_doc_comment`), mirroring
/// `TsObjectEntry::Method.doc`'s own identical split (#1337).
#[test]
fn prints_a_generic_documented_interface_method() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::Interface {
            name: "Clock".to_string(),
            type_params: Vec::new(),
            members: vec![TsTypeMember::Method {
                name: "now".to_string(),
                generics: vec!["T".to_string()],
                params: vec![TsParam::new("unit".to_string(), Some(TsType::named("T")))],
                ret: TsType::named("number"),
                doc: Some("Returns the current time.".to_string()),
            }],
        })),
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "export interface Clock {\n  /**\n   * Returns the current time.\n   */\n  now<T>(unit: T): number;\n}\n"
    );
}

/// #1359's own real need: `print_class_method` — a class-method sibling
/// of `print_object_entry`'s own fragment-printing entry point,
/// `emit_provider`'s own hand-written class wrapper prints each real
/// method through this directly. `depth` means the *class's* own
/// depth, so at `depth == 0` the method itself lands at `indent(1)`,
/// its body at `indent(2)`, matching `TsDecl::Class`'s own equivalent
/// per-method indent exactly — but with no automatic blank-line
/// insertion of its own (unlike `TsDecl::Class`'s own render arm),
/// since `emit_provider`'s own real spacing has none between methods.
#[test]
fn prints_a_single_class_method_fragment() {
    let method = TsClassMethod {
        name: "double".to_string(),
        private: false,
        is_async: true,
        params: vec![TsParam::new("n".to_string(), Some(TsType::named("number")))],
        return_type: Some(TsType::named("number")),
        doc: None,
        body: vec![TsStmt::return_stmt(
            Some(TsExpr::Binary {
                op: TsBinaryOp::NullishCoalescing,
                left: Box::new(TsExpr::Ident("n".to_string())),
                right: Box::new(TsExpr::Lit(TsLit::Num("0".to_string()))),
            }),
            None,
        )],
        span: None,
    };
    assert_eq!(
        print_class_method(&method, 0),
        "  async double(n: number): number {\n    return n ?? 0;\n  }\n"
    );
}

/// Arc C, slice 21 (#1371): `TsClassMethod.private` — pins both the
/// keyword itself and its render-*before*-`async` ordering directly,
/// the same precedent `TsClassField.private` already has (its own
/// direct test, not left to transitive `bynk` fixture coverage alone)
/// — review of #1372 caught this one still missing for the sibling
/// field.
#[test]
fn prints_a_private_class_method_fragment() {
    let method = TsClassMethod {
        name: "loadState".to_string(),
        private: true,
        is_async: true,
        params: Vec::new(),
        return_type: Some(TsType::named_with_args(
            "Promise",
            vec![TsType::named("OrderState")],
        )),
        doc: None,
        body: vec![TsStmt::return_stmt(
            Some(TsExpr::Ident("state".to_string())),
            None,
        )],
        span: None,
    };
    assert_eq!(
        print_class_method(&method, 0),
        "  private async loadState(): Promise<OrderState> {\n    return state;\n  }\n"
    );
}

/// Arc C, slice 23 (#1375): `TsClassMethod.doc` — the grounding pass's
/// own second predicted gap (#1366), the same need
/// `TsObjectEntry::Method.doc` (#1337) and `TsTypeMember::Method.doc`
/// (#1357) already solved for their own node kinds, closed here for the
/// third and last real method-shaped node in this crate.
#[test]
fn prints_a_documented_class_method_fragment() {
    let method = TsClassMethod {
        name: "spend".to_string(),
        private: false,
        is_async: false,
        params: Vec::new(),
        return_type: Some(TsType::named("void")),
        doc: Some("Debits the account.".to_string()),
        body: vec![],
        span: None,
    };
    assert_eq!(
        print_class_method(&method, 0),
        "  /**\n   * Debits the account.\n   */\n  spend(): void {\n  }\n"
    );
}

/// #1339's own real gap: `TsExpr::Arrow` had no `generics`/`return_type`
/// field — `emit_sum_type`'s own generic payload-constructor arrows
/// (`<T>(name: T): Sum<T> => (...)`) need both. The object-literal body
/// is wrapped in an explicit `Paren` — `Arrow`'s own renderer does not
/// auto-parenthesise an object body the way real JS/TS syntax requires
/// to disambiguate it from a block.
#[test]
fn prints_a_generic_arrow_with_a_parenthesised_object_body() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Arrow {
            params: vec![TsParam::new("value".to_string(), Some(TsType::named("T")))],
            is_async: false,
            generics: vec!["T".to_string()],
            return_type: Some(TsType::named_with_args("Sum", vec![TsType::named("T")])),
            body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Paren(Box::new(
                TsExpr::object_entries(vec![
                    TsObjectEntry::prop(
                        "tag".to_string(),
                        TsExpr::Lit(TsLit::Str("some".to_string())),
                    ),
                    TsObjectEntry::Shorthand("value".to_string()),
                ]),
            ))))),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "<T>(value: T): Sum<T> => ({ tag: \"some\", value });\n"
    );
}

/// Arc C, slice 33 (#1401): `TsBinaryOp::LessThan` — pins the operator
/// text itself.
#[test]
fn prints_a_less_than_comparison() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::LessThan,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Ident("b".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a < b;\n");
}

/// `LessThan` shares `GreaterThan`'s own precedence tier — a nested
/// `Add` under it needs no parens (`+` binds tighter), mirroring
/// `add_binds_tighter_than_greater_than_on_both_sides` for the other
/// relational operator.
#[test]
fn add_binds_tighter_than_less_than() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::LessThan,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            }),
            right: Box::new(TsExpr::Ident("c".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a + b < c;\n");
}

/// Arc C, slice 34 (`tests_emit.rs` slice D, #1403): `TsBinaryOp::
/// InstanceOf` — pins the keyword operator text itself, sharing
/// `LessThan`/`GreaterThan`'s own precedence tier.
#[test]
fn prints_an_instanceof_check() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::InstanceOf,
            left: Box::new(TsExpr::Ident("e".to_string())),
            right: Box::new(TsExpr::Ident("ExpectationError".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "e instanceof ExpectationError;\n");
}

/// Review of #1404, finding 2: `prints_an_instanceof_check` alone pins
/// the operator text but not the precedence tier the same diff added to
/// `binary_precedence` — it passes identically whether `InstanceOf` sits
/// at tier 5, tier 1, or is missing from that arm entirely. `Add` (tier
/// 6) binds tighter, mirroring `add_binds_tighter_than_less_than`
/// for the other relational operator sharing this tier.
#[test]
fn add_binds_tighter_than_instanceof() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::InstanceOf,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            }),
            right: Box::new(TsExpr::Ident("C".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a + b instanceof C;\n");
}

/// Review of #1404, finding 2 (the more valuable half): a catch clause
/// naturally grows into `e instanceof A || e instanceof B` as a second
/// error type is added. `InstanceOf` (tier 5) binds tighter than `Or`
/// (tier 2), so this must print flat — a wrong tier would silently
/// over-parenthesize it into `(e instanceof A) || (e instanceof B)`.
#[test]
fn instanceof_binds_tighter_than_or_on_both_sides() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::Or,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::InstanceOf,
                left: Box::new(TsExpr::Ident("e".to_string())),
                right: Box::new(TsExpr::Ident("A".to_string())),
            }),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::InstanceOf,
                left: Box::new(TsExpr::Ident("e".to_string())),
                right: Box::new(TsExpr::Ident("B".to_string())),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "e instanceof A || e instanceof B;\n");
}

/// Arc E slice 5 (`serialisation.rs`, #1443): `TsBinaryOp::In` — pins
/// the keyword operator text itself, sharing `LessThan`/`GreaterThan`/
/// `InstanceOf`'s own precedence tier.
#[test]
fn prints_an_in_check() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::In,
            left: Box::new(TsExpr::Lit(TsLit::Str("name".to_string()))),
            right: Box::new(TsExpr::Ident("obj".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "\"name\" in obj;\n");
}

/// Mirrors `add_binds_tighter_than_instanceof`/`add_binds_tighter_than_
/// less_than` for the newest member of this shared precedence tier:
/// `Add` (tier 6) binds tighter than `In` (tier 5).
#[test]
fn add_binds_tighter_than_in() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::In,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            }),
            right: Box::new(TsExpr::Ident("obj".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a + b in obj;\n");
}

/// Mirrors `instanceof_binds_tighter_than_or_on_both_sides`: `In` (tier
/// 5) binds tighter than `Or` (tier 2), so a real
/// `"a" in obj || "b" in obj` reads flat with no parens — a wrong tier
/// would silently over-parenthesize it.
#[test]
fn in_binds_tighter_than_or_on_both_sides() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::Or,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::In,
                left: Box::new(TsExpr::Lit(TsLit::Str("a".to_string()))),
                right: Box::new(TsExpr::Ident("obj".to_string())),
            }),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::In,
                left: Box::new(TsExpr::Lit(TsLit::Str("b".to_string()))),
                right: Box::new(TsExpr::Ident("obj".to_string())),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "\"a\" in obj || \"b\" in obj;\n");
}

/// Review of #1471, finding 1: `TsBinaryOp::GreaterThanEq`/`LessThanEq`
/// landed with no direct `bynk-ts` test — only pinned indirectly through
/// `bynk-emit`'s own fixture goldens. This test and the four below close
/// that locality gap, mirroring the coverage every other operator on
/// this precedence tier already has (`prints_an_in_check`/`add_binds_
/// tighter_than_in`/`in_binds_tighter_than_or_on_both_sides`).
#[test]
fn prints_a_greater_than_or_equal_comparison() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::GreaterThanEq,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Ident("b".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a >= b;\n");
}

#[test]
fn prints_a_less_than_or_equal_comparison() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::LessThanEq,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Ident("b".to_string())),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a <= b;\n");
}

/// Pins `GreaterThanEq`/`LessThanEq`'s own tier-5 placement — the real
/// `pred_condition_and_message` `InRange` shape (#1471) nests both under
/// `And` (tier 3) and must print flat, with no parens on either side. A
/// regression that demoted either operator to `And`'s own tier or below
/// would silently wrap this in parens instead.
#[test]
fn greater_than_eq_and_less_than_eq_nest_flat_under_and() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::And,
            left: Box::new(TsExpr::Binary {
                op: TsBinaryOp::GreaterThanEq,
                left: Box::new(TsExpr::Ident("value".to_string())),
                right: Box::new(TsExpr::Lit(TsLit::Num("0".to_string()))),
            }),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::LessThanEq,
                left: Box::new(TsExpr::Ident("value".to_string())),
                right: Box::new(TsExpr::Lit(TsLit::Num("100".to_string()))),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "value >= 0 && value <= 100;\n");
}

/// Same tier, *different* operator: `render_binary_operand`'s own doc
/// says equal precedence still parenthesises unless the two sides are
/// the exact same associative operator — `GreaterThan`/`GreaterThanEq`
/// are equal-tier but distinct, so nesting one under the other must
/// still keep its parens (the `_ => true` fallback of the equal-
/// precedence match, newly reachable now this tier has more than one
/// non-keyword operator).
#[test]
fn a_greater_than_eq_operand_of_a_different_relational_op_still_parenthesises() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::GreaterThan,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::GreaterThanEq,
                left: Box::new(TsExpr::Ident("b".to_string())),
                right: Box::new(TsExpr::Ident("c".to_string())),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a > (b >= c);\n");
}

/// `GreaterThanEq`/`LessThanEq` nested in themselves are NOT associative
/// (`a >= b >= c` does not parse as `a >= (b >= c)`), unlike `||`/`&&` —
/// so, unlike `in_binds_tighter_than_or_on_both_sides`'s own flat `In`
/// chain, a same-operator nesting here must still parenthesise.
#[test]
fn a_right_nested_greater_than_eq_chain_keeps_its_parens() {
    let mut program = TsProgram::new();
    program.push(TsStmt::expr_stmt(
        TsExpr::Binary {
            op: TsBinaryOp::GreaterThanEq,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Binary {
                op: TsBinaryOp::GreaterThanEq,
                left: Box::new(TsExpr::Ident("b".to_string())),
                right: Box::new(TsExpr::Ident("c".to_string())),
            }),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(printed.text, "a >= (b >= c);\n");
}

// --- `print_stmt_with_spans` (back-half paydown 1A.11) ---

fn id(name: &str) -> TsExpr {
    TsExpr::Ident(name.to_string())
}

fn sp(start: usize) -> Option<Span> {
    Some(Span::new(start, start + 1))
}

/// Each reported offset is the start of the statement's own line, before
/// its indentation.
fn line_at(text: &str, offset: usize) -> &str {
    text[offset..].lines().next().expect("a line")
}

#[test]
fn spans_of_a_flat_statement_are_its_own_start() {
    let stmt = TsStmt::return_stmt(Some(id("x")), sp(7));
    let (text, spans) = print_stmt_with_spans(&stmt, 0);
    assert_eq!(text, "return x;\n");
    assert_eq!(spans, vec![(0, Span::new(7, 8))]);
    // At a depth the offset is still before the indentation.
    let (text, spans) = print_stmt_with_spans(&stmt, 2);
    assert_eq!(text, "    return x;\n");
    assert_eq!(spans, vec![(0, Span::new(7, 8))]);
}

#[test]
fn a_statement_without_a_span_is_not_reported() {
    let stmt = TsStmt::return_stmt(Some(id("x")), None);
    assert!(print_stmt_with_spans(&stmt, 0).1.is_empty());
}

#[test]
fn spans_of_nested_blocks_are_reported_in_printed_order() {
    let inner = TsStmt::if_else_same_line_stmt(
        id("b"),
        TsStmt::block(
            vec![TsStmt::return_stmt(
                Some(TsExpr::Lit(TsLit::Num("1".to_string()))),
                sp(40),
            )],
            None,
        ),
        TsStmt::block(
            vec![TsStmt::return_stmt(
                Some(TsExpr::Lit(TsLit::Num("2".to_string()))),
                sp(50),
            )],
            None,
        ),
        sp(30),
    );
    let outer = TsStmt::if_else_same_line_stmt(
        id("a"),
        TsStmt::block(
            vec![
                TsStmt::const_stmt(TsBindingName::Ident("v".into()), None, id("w"), sp(10)),
                inner,
            ],
            None,
        ),
        TsStmt::block(
            vec![TsStmt::return_stmt(
                Some(TsExpr::Lit(TsLit::Num("3".to_string()))),
                sp(60),
            )],
            None,
        ),
        None,
    );
    let (text, spans) = print_stmt_with_spans(&outer, 0);
    assert_eq!(
        text,
        "if (a) {\n  const v = w;\n  if (b) {\n    return 1;\n  } else {\n    return 2;\n  }\n} else {\n  return 3;\n}\n"
    );
    let starts: Vec<usize> = spans.iter().map(|(_, s)| s.start).collect();
    assert_eq!(starts, [10, 30, 40, 50, 60]);
    let lines: Vec<&str> = spans.iter().map(|(o, _)| line_at(&text, *o)).collect();
    assert_eq!(
        lines,
        [
            "  const v = w;",
            "  if (b) {",
            "    return 1;",
            "    return 2;",
            "  return 3;"
        ]
    );
}

#[test]
fn an_inline_branch_starts_after_its_separating_space() {
    let stmt = TsStmt::if_else_stmt(
        id("c"),
        TsStmt::return_stmt(Some(TsExpr::Lit(TsLit::Num("1".to_string()))), sp(5)),
        TsStmt::return_stmt(Some(TsExpr::Lit(TsLit::Num("2".to_string()))), sp(6)),
        sp(4),
    );
    let (text, spans) = print_stmt_with_spans(&stmt, 0);
    assert_eq!(text, "if (c) return 1;\nelse return 2;\n");
    let at: Vec<(&str, usize)> = spans
        .iter()
        .map(|(o, s)| (&text[*o..*o + 8.min(text.len() - o)], s.start))
        .collect();
    assert_eq!(at, [("if (c) r", 4), ("return 1", 5), ("return 2", 6)]);
}

#[test]
fn a_multi_line_node_reports_offsets_that_count_every_line() {
    let stmt = TsStmt::block(
        vec![
            TsStmt::if_stmt(
                id("x"),
                TsStmt::block(vec![TsStmt::expr_stmt(id("a"), sp(1))], None),
                sp(0),
            ),
            TsStmt::expr_stmt(id("b"), sp(2)),
        ],
        None,
    );
    let (text, spans) = print_stmt_with_spans(&stmt, 0);
    let lines: Vec<(&str, usize)> = spans
        .iter()
        .map(|(o, s)| (line_at(&text, *o), s.start))
        .collect();
    assert_eq!(lines, [("  if (x) {", 0), ("    a;", 1), ("  b;", 2)]);
}

#[test]
fn other_printing_records_nothing_and_the_sink_is_restored() {
    let stmt = TsStmt::return_stmt(Some(id("x")), sp(7));
    let _ = print_stmt_with_spans(&stmt, 0);
    // A plain print afterwards must not leave anything behind.
    let _ = print_stmt(&stmt, 0);
    assert!(
        print_stmt_with_spans(&TsStmt::continue_stmt(None), 0)
            .1
            .is_empty()
    );
    let again = print_stmt_with_spans(&stmt, 0);
    assert_eq!(again.1.len(), 1);
}

fn multiline_arrow(is_async: bool, stmts: Vec<TsStmt>) -> TsExpr {
    TsExpr::Arrow {
        params: Vec::new(),
        is_async,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Multiline(stmts)),
    }
}

fn iife(arrow: TsExpr) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(TsExpr::Paren(Box::new(arrow))),
        args: Vec::new(),
    }
}

/// 1A.12: an object-literal expression body is bracketed, or it would read
/// as a block.
#[test]
fn an_object_literal_expression_body_is_bracketed() {
    let arrow = TsExpr::Arrow {
        params: vec![TsParam::new("x".to_string(), None)],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::object(vec![(
            "a".to_string(),
            id("x"),
        )])))),
    };
    assert_eq!(print_expr(&arrow), "(x) => ({ a: x })");
}

/// 1A.12: a multi-line arrow body nests one level below the line the arrow
/// starts on, and closes back at that line's indent, at any depth.
#[test]
fn a_multiline_arrow_body_prints_at_its_real_depth() {
    let call = iife(multiline_arrow(
        false,
        vec![
            TsStmt::const_stmt(TsBindingName::Ident("y".to_string()), None, id("a"), None),
            TsStmt::return_stmt(Some(id("y")), None),
        ],
    ));
    let stmt = TsStmt::const_stmt(TsBindingName::Ident("r".to_string()), None, call, None);
    assert_eq!(
        print_stmt(&stmt, 0),
        "const r = (() => {\n  const y = a;\n  return y;\n})();\n"
    );
    assert_eq!(
        print_stmt(&stmt, 1),
        "  const r = (() => {\n    const y = a;\n    return y;\n  })();\n"
    );
}

/// 1A.12: three IIFEs deep, each body one level below its own line.
#[test]
fn nested_multiline_arrows_indent_one_level_each() {
    let inner = iife(multiline_arrow(
        false,
        vec![TsStmt::return_stmt(Some(id("z")), None)],
    ));
    let middle = iife(multiline_arrow(
        false,
        vec![TsStmt::return_stmt(Some(inner), None)],
    ));
    let outer = iife(multiline_arrow(
        true,
        vec![TsStmt::return_stmt(Some(middle), None)],
    ));
    let stmt = TsStmt::return_stmt(Some(TsExpr::Await(Box::new(outer))), None);
    assert_eq!(
        print_stmt(&stmt, 0),
        "return await (async () => {\n  return (() => {\n    return (() => {\n      return z;\n    })();\n  })();\n})();\n"
    );
}

/// 1A.12: a multi-line arrow inside a call argument sits at the enclosing
/// statement's depth.
#[test]
fn a_multiline_arrow_as_an_argument_uses_the_statement_depth() {
    let lambda = TsExpr::Arrow {
        params: vec![TsParam::new("x".to_string(), None)],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Multiline(vec![TsStmt::return_stmt(
            Some(id("x")),
            None,
        )])),
    };
    let call = TsExpr::Call {
        callee: Box::new(id("f")),
        args: vec![id("xs"), lambda],
    };
    assert_eq!(
        print_stmt(&TsStmt::expr_stmt(call, None), 2),
        "    f(xs, (x) => {\n      return x;\n    });\n"
    );
}

/// 1A.15: statements inside a multi-line arrow record their spans, at the
/// start of their own line, after the statement that holds the arrow.
#[test]
fn a_multiline_arrow_body_records_its_statement_spans() {
    let call = iife(multiline_arrow(
        false,
        vec![TsStmt::return_stmt(Some(id("x")), sp(9))],
    ));
    let stmt = TsStmt::expr_stmt(call, sp(3));
    let (text, spans) = print_stmt_with_spans(&stmt, 0);
    let lines: Vec<(&str, usize)> = spans
        .iter()
        .map(|(o, s)| (line_at(&text, *o), s.start))
        .collect();
    assert_eq!(lines, [("(() => {", 3), ("  return x;", 9)]);
}

/// 1A.15: a lambda passed as an argument is mapped too.
#[test]
fn a_lambda_argument_body_records_its_statement_spans() {
    let lambda = TsExpr::Arrow {
        params: Vec::new(),
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Multiline(vec![TsStmt::return_stmt(
            Some(id("x")),
            sp(8),
        )])),
    };
    let call = TsExpr::Call {
        callee: Box::new(id("f")),
        args: vec![id("xs"), lambda],
    };
    let stmt = TsStmt::return_stmt(Some(call), sp(2));
    let (text, spans) = print_stmt_with_spans(&stmt, 1);
    let lines: Vec<(&str, usize)> = spans
        .iter()
        .map(|(o, s)| (line_at(&text, *o), s.start))
        .collect();
    assert_eq!(lines, [("  return f(xs, () => {", 2), ("    return x;", 8)]);
}

/// 1A.15 review (#3757): `render_multiline_object` now passes the
/// caller's map to its entries, so a `Method` entry of a multiline object
/// in statement position records its body's statement spans. No producer
/// gives those bodies spans today (`__makeSurface` and the workers
/// wrappers build them with `span: None`); this pins the behaviour so a
/// caller that also merges a method body's map by hand is caught.
#[test]
fn a_multiline_object_method_body_records_its_statement_spans() {
    let entry = TsObjectEntry::Method {
        name: "m".to_string(),
        is_async: false,
        generics: Vec::new(),
        params: Vec::new(),
        return_type: None,
        doc: None,
        inline: false,
        body: vec![TsStmt::return_stmt(Some(id("x")), sp(7))],
        span: None,
    };
    let object = TsExpr::Object {
        entries: vec![entry],
        multiline: true,
        end_span: None,
    };
    let stmt = TsStmt::return_stmt(Some(object), sp(3));
    let (text, spans) = print_stmt_with_spans(&stmt, 1);
    let lines: Vec<(&str, usize)> = spans
        .iter()
        .map(|(o, s)| (line_at(&text, *o), s.start))
        .collect();
    assert_eq!(lines, [("  return {", 3), ("      return x;", 7)], "{text}");
}

// --- `TsSwitchCase.labels` and `.span` (back-half paydown 1A.13) ---

fn case(labels: Vec<TsExpr>, body: Vec<TsStmt>, span: Option<Span>) -> TsSwitchCase {
    let default = labels.is_empty();
    TsSwitchCase {
        labels,
        body,
        default_braced: default,
        case_braced: true,
        span,
    }
}

fn lit(s: &str) -> TsExpr {
    TsExpr::Lit(TsLit::Str(s.to_string()))
}

/// 1A.13: labels sharing one body print stacked, the brace on the last, as
/// an or-pattern's `match` does today.
#[test]
fn shared_labels_print_stacked_with_the_brace_on_the_last() {
    let stmt = TsStmt::switch_stmt(
        id("__d"),
        vec![
            case(
                vec![lit("A"), lit("B"), lit("C")],
                vec![TsStmt::return_stmt(
                    Some(TsExpr::Lit(TsLit::Num("1".to_string()))),
                    None,
                )],
                None,
            ),
            case(
                vec![],
                vec![TsStmt::return_stmt(
                    Some(TsExpr::Lit(TsLit::Num("2".to_string()))),
                    None,
                )],
                None,
            ),
        ],
        None,
    );
    assert_eq!(
        print_stmt(&stmt, 0),
        "switch (__d) {\n  case \"A\":\n  case \"B\":\n  case \"C\": {\n    return 1;\n  }\n  default: {\n    return 2;\n  }\n}\n"
    );
}

/// 1A.13: shared labels over an unbraced body stack the same way.
#[test]
fn shared_labels_over_an_unbraced_body() {
    let mut shared = case(
        vec![lit("A"), lit("B")],
        vec![TsStmt::return_stmt(
            Some(TsExpr::Lit(TsLit::Num("1".to_string()))),
            None,
        )],
        None,
    );
    shared.case_braced = false;
    let stmt = TsStmt::switch_stmt(id("k"), vec![shared], None);
    assert_eq!(
        print_stmt(&stmt, 0),
        "switch (k) {\n  case \"A\":\n  case \"B\":\n    return 1;\n}\n"
    );
}

/// 1A.13: a case's span is recorded at the start of its first label line,
/// before its indentation, ahead of the spans inside it, at any depth.
#[test]
fn a_case_span_is_recorded_at_its_first_label_line() {
    let stmt = TsStmt::switch_stmt(
        id("k"),
        vec![
            case(
                vec![lit("A"), lit("B")],
                vec![TsStmt::return_stmt(
                    Some(TsExpr::Lit(TsLit::Num("1".to_string()))),
                    sp(11),
                )],
                sp(10),
            ),
            case(
                vec![],
                vec![TsStmt::return_stmt(
                    Some(TsExpr::Lit(TsLit::Num("2".to_string()))),
                    None,
                )],
                sp(20),
            ),
        ],
        sp(5),
    );
    let (text, spans) = print_stmt_with_spans(&stmt, 0);
    let lines: Vec<(&str, usize)> = spans
        .iter()
        .map(|(o, s)| (line_at(&text, *o), s.start))
        .collect();
    assert_eq!(
        lines,
        [
            ("switch (k) {", 5),
            ("  case \"A\":", 10),
            ("    return 1;", 11),
            ("  default: {", 20),
        ]
    );
    // The offset is before the indentation at a depth too.
    let (text, spans) = print_stmt_with_spans(&stmt, 2);
    assert_eq!(line_at(&text, spans[1].0), "      case \"A\":");
    assert_eq!(spans[1].0, text.find("      case").unwrap());
}

/// 1A.13: a case without a span records nothing, and plain printing never
/// does.
#[test]
fn a_spanless_case_records_nothing() {
    let stmt = TsStmt::switch_stmt(id("k"), vec![case(vec![lit("A")], vec![], None)], None);
    assert!(print_stmt_with_spans(&stmt, 0).1.is_empty());
}

/// 1A.13 (review of #3750): a multi-line arrow reaching a spread entry or a
/// multi-line array item sits one level below the line the entry is on.
#[test]
fn a_multiline_arrow_in_a_spread_entry_or_array_item_prints_at_its_depth() {
    let spread = TsObjectEntry::Spread(iife(multiline_arrow(
        false,
        vec![TsStmt::return_stmt(Some(id("x")), None)],
    )));
    assert_eq!(
        print_object_entry(&spread, 0),
        "  ...(() => {\n    return x;\n  })(),\n"
    );
    let array = TsExpr::Array {
        items: vec![iife(multiline_arrow(
            false,
            vec![TsStmt::return_stmt(Some(id("x")), None)],
        ))],
        multiline: true,
    };
    let stmt = TsStmt::const_stmt(TsBindingName::Ident("a".to_string()), None, array, None);
    assert_eq!(
        print_stmt(&stmt, 0),
        "const a = [\n  (() => {\n    return x;\n  })(),\n];\n"
    );
}
/// 1A.14 (Decision B of #3715, which asked for an `InlineBlock` arrow body):
/// the compact `{ s1; s2; }` form already exists as [`TsArrowBody::Block`],
/// so the short-circuit IIFEs use it. It prints on one line with a `const`
/// and a `return`, inside brackets and a call.
#[test]
fn a_compact_arrow_body_prints_a_const_and_a_return_on_one_line() {
    let arrow = TsExpr::Arrow {
        params: Vec::new(),
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Block(vec![
            TsStmt::const_stmt(
                TsBindingName::Ident("n".to_string()),
                None,
                TsExpr::Member {
                    object: Box::new(id("r")),
                    property: "value".to_string(),
                },
                None,
            ),
            TsStmt::return_stmt(Some(id("n")), None),
        ])),
    };
    let call = TsExpr::Paren(Box::new(iife(arrow)));
    let and = TsExpr::Binary {
        op: TsBinaryOp::And,
        left: Box::new(id("ok")),
        right: Box::new(call),
    };
    assert_eq!(
        print_expr(&and),
        "ok && ((() => { const n = r.value; return n; })())"
    );
}

/// 1A.14: an array-destructuring pattern; an empty name is a hole.
#[test]
fn an_array_pattern_prints_holes_as_empty_entries() {
    let init = TsExpr::Await(Box::new(id("p")));
    let named = TsStmt::const_stmt(
        TsBindingName::ArrayPattern(vec!["a".to_string(), "b".to_string()]),
        None,
        init.clone(),
        None,
    );
    assert_eq!(print_stmt(&named, 0), "const [a, b] = await p;\n");
    let hole = TsStmt::const_stmt(
        TsBindingName::ArrayPattern(vec![String::new(), "b".to_string()]),
        None,
        init,
        None,
    );
    assert_eq!(print_stmt(&hole, 0), "const [, b] = await p;\n");
}

/// 1A.14: `par`'s join: thunks with a return annotation, `async` when asked,
/// in a lines array (one item per line) under an `await` call.
#[test]
fn typed_and_async_thunks_print_in_a_multiline_array() {
    let thunk = |is_async: bool, ret: Option<TsType>, body: TsArrowBody| TsExpr::Arrow {
        params: Vec::new(),
        is_async,
        generics: Vec::new(),
        return_type: ret,
        body: Box::new(body),
    };
    let promise = TsType::named_with_args("globalThis.Promise", vec![TsType::named("number")]);
    let items = vec![
        thunk(false, Some(promise), TsArrowBody::Expr(Box::new(id("a")))),
        thunk(
            true,
            None,
            TsArrowBody::Multiline(vec![
                TsStmt::const_stmt(TsBindingName::Ident("t".to_string()), None, id("x"), None),
                TsStmt::return_stmt(Some(id("t")), None),
            ]),
        ),
    ];
    let call = TsExpr::Await(Box::new(TsExpr::Call {
        callee: Box::new(id("__join")),
        args: vec![TsExpr::LinesArray(items)],
    }));
    let stmt = TsStmt::const_stmt(
        TsBindingName::ArrayPattern(vec!["a".to_string(), "b".to_string()]),
        None,
        call,
        None,
    );
    assert_eq!(
        print_stmt(&stmt, 1),
        "  const [a, b] = await __join([\n    (): globalThis.Promise<number> => a,\n    async () => {\n      const t = x;\n      return t;\n    },\n  ]);\n"
    );
}
// --- nested spans through the program printer (1A.15, Decision B of #3716) ---

fn function(body: Vec<TsStmt>) -> TsDecl {
    TsDecl::Function {
        name: "f".to_string(),
        generics: Vec::new(),
        params: Vec::new(),
        return_type: None,
        body,
        is_async: false,
        inline: false,
    }
}

/// Every statement of a function body records its span, at any depth,
/// through `print` itself: `print_with_map` no longer stops at the top level.
#[test]
fn a_program_records_nested_statement_spans_at_any_depth() {
    let body = vec![
        TsStmt::const_stmt(TsBindingName::Ident("a".into()), None, id("x"), sp(10)),
        TsStmt::if_stmt(
            id("c"),
            TsStmt::block(vec![TsStmt::return_stmt(Some(id("a")), sp(30))], sp(20)),
            sp(15),
        ),
    ];
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(function(body), None));
    let mut map = SourceMapBuilder::new();
    map.add_source("x.bynk", " ".repeat(100));
    let printed = print_multi_source(&program, map, "x.ts");
    assert_eq!(
        printed.text,
        "function f() {\n  const a = x;\n  if (c) {\n    return a;\n  }\n}\n"
    );
    // The same program, printed through the fragment entry point, shows
    // where each checkpoint falls.
    let mut out = String::new();
    let mut map = SourceMapBuilder::new();
    let decl = TsStmt::decl(
        function(vec![
            TsStmt::const_stmt(TsBindingName::Ident("a".into()), None, id("x"), sp(10)),
            TsStmt::if_stmt(
                id("c"),
                TsStmt::block(vec![TsStmt::return_stmt(Some(id("a")), sp(30))], sp(20)),
                sp(15),
            ),
        ]),
        None,
    );
    print_stmt_and_merge(&mut out, &decl, 0, &mut map, 0);
    let spans = map.checkpoint_spans();
    let starts: Vec<usize> = spans.iter().map(|(_, s)| s.start).collect();
    assert_eq!(starts, [10, 15, 30]);
    let lines: Vec<&str> = spans.iter().map(|(o, _)| line_at(&out, *o)).collect();
    assert_eq!(lines, ["  const a = x;", "  if (c) {", "    return a;"]);
}

/// A class method's body records its statements' spans too.
#[test]
fn a_class_method_records_its_body_statement_spans() {
    let method = TsClassMethod {
        name: "m".to_string(),
        private: false,
        is_async: false,
        params: Vec::new(),
        return_type: None,
        doc: None,
        body: vec![
            TsStmt::const_stmt(TsBindingName::Ident("a".into()), None, id("x"), sp(5)),
            TsStmt::return_stmt(Some(id("a")), sp(9)),
        ],
        span: None,
    };
    let mut out = String::from("class C {\n");
    let mut map = SourceMapBuilder::new();
    print_class_method_and_merge(&mut out, &method, 0, &mut map, 0);
    let spans = map.checkpoint_spans();
    let lines: Vec<&str> = spans.iter().map(|(o, _)| line_at(&out, *o)).collect();
    assert_eq!(lines, ["    const a = x;", "    return a;"]);
    let starts: Vec<usize> = spans.iter().map(|(_, s)| s.start).collect();
    assert_eq!(starts, [5, 9]);
}

/// A `nested_map` on one body statement still merges, next to spans the
/// statements around it carry.
#[test]
fn a_nested_map_merges_next_to_nested_spans() {
    let mut sub = SourceMapBuilder::new();
    sub.record(0, Span::new(41, 42));
    let mut raw = TsStmt::raw("    carried();\n    more();\n", None);
    raw.nested_map = Some(sub);
    let body = vec![
        TsStmt::const_stmt(TsBindingName::Ident("a".into()), None, id("x"), sp(10)),
        raw,
        TsStmt::return_stmt(Some(id("a")), sp(60)),
    ];
    let mut out = String::new();
    let mut map = SourceMapBuilder::new();
    let decl = TsStmt::decl(function(body), None);
    print_stmt_and_merge(&mut out, &decl, 0, &mut map, 0);
    let spans = map.checkpoint_spans();
    let starts: Vec<usize> = spans.iter().map(|(_, s)| s.start).collect();
    assert_eq!(starts, [10, 41, 60]);
    let lines: Vec<&str> = spans.iter().map(|(o, _)| line_at(&out, *o)).collect();
    assert_eq!(lines, ["  const a = x;", "    carried();", "  return a;"]);
}

/// A `nested_map` on a statement inside an arrow body merges next to the
/// spans around it, at the statement's real offset (1A.15, Decision D).
#[test]
fn a_nested_map_inside_an_arrow_body_merges_at_its_real_offset() {
    let mut sub = SourceMapBuilder::new();
    sub.record(0, Span::new(41, 42));
    let mut raw = TsStmt::raw("    carried();\n", None);
    raw.nested_map = Some(sub);
    let arrow = multiline_arrow(false, vec![TsStmt::return_stmt(Some(id("a")), sp(7)), raw]);
    let stmt = TsStmt::expr_stmt(iife(arrow), sp(3));
    let mut out = String::new();
    let mut map = SourceMapBuilder::new();
    print_stmt_and_merge(&mut out, &stmt, 0, &mut map, 0);
    let spans = map.checkpoint_spans();
    let lines: Vec<(&str, usize)> = spans
        .iter()
        .map(|(o, s)| (line_at(&out, *o), s.start))
        .collect();
    assert_eq!(
        lines,
        [("(() => {", 3), ("  return a;", 7), ("    carried();", 41)]
    );
}

/// Nothing is recorded when the print has no map to record into.
#[test]
fn a_print_without_a_map_records_nothing() {
    let decl = TsStmt::decl(
        function(vec![TsStmt::return_stmt(Some(id("a")), sp(9))]),
        sp(1),
    );
    assert_eq!(print_stmt(&decl, 0), "function f() {\n  return a;\n}\n");
}

/// A class with `implements` prints the clause after its name, one type or several; an empty
/// vector prints as before (1A.32a).
#[test]
fn a_class_prints_its_implements_clause() {
    let class = |implements: Vec<TsType>| {
        print_stmt(
            &TsStmt::decl(
                TsDecl::class("C", Vec::new(), None, Vec::new()).implementing(implements),
                None,
            ),
            0,
        )
    };
    assert_eq!(class(Vec::new()), "class C {\n}\n");
    assert_eq!(
        class(vec![TsType::named("Cap")]),
        "class C implements Cap {\n}\n"
    );
    assert_eq!(
        class(vec![TsType::named("A"), TsType::named("B")]),
        "class C implements A, B {\n}\n"
    );
}

/// A spread is a legal call argument as well as an array item (1A.32b), printing `...` and its
/// operand wherever it sits among the arguments.
#[test]
fn a_spread_prints_as_a_call_argument() {
    let spread = |n: &str| TsExpr::Spread(Box::new(id(n)));
    let call = |args: Vec<TsExpr>| TsExpr::Call {
        callee: Box::new(id("f")),
        args,
    };
    assert_eq!(print_expr(&call(vec![spread("a")])), "f(...a)");
    assert_eq!(
        print_expr(&call(vec![id("x"), spread("rest"), id("y")])),
        "f(x, ...rest, y)"
    );
    let array = TsExpr::array(vec![id("x"), spread("rest")]);
    assert_eq!(print_expr(&array), "[x, ...rest]");
}

/// `break;` ends a case body on its own line, and prints on one line inside an inline block
/// and a compact statement list (1A.32b).
#[test]
fn a_break_statement_prints_in_block_inline_and_compact_bodies() {
    let case = |body: Vec<TsStmt>| TsSwitchCase {
        labels: vec![TsExpr::Lit(TsLit::Num("0".to_string()))],
        body,
        default_braced: false,
        case_braced: false,
        span: None,
    };
    let call = || {
        TsStmt::expr_stmt(
            TsExpr::Call {
                callee: Box::new(id("f")),
                args: Vec::new(),
            },
            None,
        )
    };
    let switch = TsStmt::switch_stmt(
        id("x"),
        vec![case(vec![call(), TsStmt::break_stmt(None)])],
        None,
    );
    assert_eq!(
        print_stmt(&switch, 0),
        "switch (x) {\n  case 0:\n    f();\n    break;\n}\n"
    );
    let inline = TsStmt::inline_block(vec![call(), TsStmt::break_stmt(None)], None);
    assert_eq!(print_stmt(&inline, 0), "{ f(); break; }\n");
    let guarded = TsStmt::if_stmt(id("c"), TsStmt::break_stmt(None), None);
    assert_eq!(print_stmt(&guarded, 0), "if (c) break;\n");
    let block = TsStmt::block(vec![TsStmt::break_stmt(None)], None);
    assert_eq!(print_stmt(&block, 0), "{\n  break;\n}\n");
}

/// `TsUnaryOp::Void` (1A.32c-1): `void x`, a bracketed binary operand, and a negated operand
/// beside it.
#[test]
fn a_void_expression_prints_with_its_operand_bracketed() {
    let void = |e: TsExpr| TsExpr::Unary {
        op: TsUnaryOp::Void,
        expr: Box::new(e),
    };
    let id = |n: &str| TsExpr::Ident(n.to_string());
    assert_eq!(print_expr(&void(id("code"))), "void code");
    assert_eq!(
        print_expr(&void(TsExpr::Binary {
            op: TsBinaryOp::Add,
            left: Box::new(id("a")),
            right: Box::new(id("b")),
        })),
        "void (a + b)"
    );
    assert_eq!(
        print_stmt(&TsStmt::expr_stmt(void(id("wasClean")), None), 1),
        "  void wasClean;\n"
    );
}

/// An object `Prop` with a `doc` prints its JSDoc block at the entry's own indent, escaping a
/// `*/` as a statement doc comment does; an undocumented one is unchanged (1A.32c-1).
#[test]
fn a_documented_object_prop_prints_its_doc_before_the_entry() {
    let obj = TsExpr::multiline_object_entries(vec![
        TsObjectEntry::prop("\"en\"", TsExpr::Lit(TsLit::Num("1".to_string())))
            .with_doc("English\n\nterms */ here"),
        TsObjectEntry::prop("\"fr\"", TsExpr::Lit(TsLit::Num("2".to_string()))),
    ]);
    let decl = TsStmt::decl(
        TsDecl::ConstDecl {
            name: "t".to_string(),
            ty: None,
            init: obj,
        },
        None,
    );
    assert_eq!(
        print_stmt(&decl, 0),
        "const t = {\n  /**\n   * English\n   *\n   * terms *\\/ here\n   */\n  \"en\": 1,\n  \"fr\": 2,\n};\n"
    );
}

/// A rest parameter prints `...name: T`, first or after others (1A.32b).
#[test]
fn a_rest_parameter_prints_its_spread() {
    let params = vec![
        TsParam::new("a", Some(TsType::named("number"))),
        TsParam::new("rest", Some(TsType::array(TsType::named("unknown")))).rest(),
    ];
    let mut out = String::new();
    render_params(&mut out, &params);
    assert_eq!(out, "a: number, ...rest: unknown[]");
    let mut only = String::new();
    render_params(
        &mut only,
        &[TsParam::new("__a", Some(TsType::array(TsType::named("unknown")))).rest()],
    );
    assert_eq!(only, "...__a: unknown[]");
}

/// A rest parameter that is also optional is invalid TypeScript, so the printer refuses it
/// loudly rather than printing `...x?`.
#[test]
#[should_panic(expected = "a rest parameter cannot be optional")]
fn a_rest_optional_parameter_is_refused() {
    let mut p = TsParam::new("xs", Some(TsType::array(TsType::named("unknown")))).rest();
    p.optional = true;
    let mut out = String::new();
    render_params(&mut out, &[p]);
}

/// A definite-assignment assertion on a field whose type admits `undefined` is refused.
#[test]
#[should_panic(expected = "definite")]
fn a_definite_optional_shaped_field_is_refused() {
    let ty = TsType::union(vec![TsType::named("K"), TsType::named("undefined")]);
    let class = TsDecl::class(
        "C",
        vec![TsClassField::new("__key", ty).definite()],
        None,
        Vec::new(),
    );
    let _ = print_stmt(&TsStmt::decl(class, None), 0);
}

/// A definite-assignment field prints `name!: T;`, with or without `private`, beside a plain
/// one; the two are independent (1A.32b).
#[test]
fn a_class_field_prints_a_definite_assignment_assertion() {
    let fields = vec![
        TsClassField::new("state", TsType::named("S")),
        TsClassField::new("__key", TsType::named("K")).definite(),
        TsClassField::new("__env", TsType::named("unknown")).private(),
        TsClassField::new("id", TsType::named("N"))
            .private()
            .definite(),
    ];
    let class = TsDecl::class("C", fields, None, Vec::new());
    assert_eq!(
        print_stmt(&TsStmt::decl(class, None), 0),
        "class C {\n  state: S;\n  __key!: K;\n  private __env: unknown;\n  private id!: N;\n}\n"
    );
}

/// A field with only an initialiser prints without a type (`__seq_0 = 0;`), as a field with
/// both prints `name: T = init;` (1B.5c).
#[test]
fn a_class_field_may_carry_an_initialiser() {
    let mut typed = TsClassField::new("n", TsType::named("number"));
    typed.init = Some(TsExpr::Lit(TsLit::Num("1".to_string())));
    let fields = vec![
        TsClassField::inferred("__seq_0", TsExpr::Lit(TsLit::Num("0".to_string()))),
        typed,
    ];
    let class = TsDecl::class("C", fields, None, Vec::new());
    assert_eq!(
        print_stmt(&TsStmt::decl(class, None), 0),
        "class C {\n  __seq_0 = 0;\n  n: number = 1;\n}\n"
    );
}

/// A class method's span maps its first printed line, its doc comment when it has one, and the
/// class's `end_span` maps the closing brace line, not the last method's final body line
/// (1A.32a). A `nested_map` on a body statement merges beside the member span.
#[test]
fn a_class_records_member_and_closing_spans_beside_a_nested_map() {
    let mut sub = SourceMapBuilder::new();
    sub.record(0, Span::new(41, 42));
    let mut raw = TsStmt::raw("    carried();\n", None);
    raw.nested_map = Some(sub);
    let method = |name: &str, doc: Option<&str>, span: usize, body: Vec<TsStmt>| TsClassMethod {
        doc: doc.map(str::to_owned),
        span: sp(span),
        ..TsClassMethod::new(name, false, Vec::new(), None, body)
    };
    let class = TsDecl::class(
        "C",
        Vec::new(),
        None,
        vec![
            method("a", Some("Does a."), 10, vec![raw]),
            method(
                "b",
                None,
                20,
                vec![TsStmt::return_stmt(Some(id("x")), sp(25))],
            ),
        ],
    )
    .ending_at(Span::new(90, 91));
    let mut out = String::new();
    let mut map = SourceMapBuilder::new();
    print_stmt_and_merge(&mut out, &TsStmt::decl(class, None), 0, &mut map, 0);
    let lines: Vec<(&str, usize)> = map
        .checkpoint_spans()
        .iter()
        .map(|(o, s)| (line_at(&out, *o), s.start))
        .collect();
    assert_eq!(
        lines,
        [
            ("  /**", 10),
            ("    carried();", 41),
            ("  b() {", 20),
            ("    return x;", 25),
            ("}", 90),
        ]
    );
}

/// An object entry's span and a multiline object's `end_span` record the same way, through a
/// top-level `const` (1A.32a); a nested map still merges beside them.
#[test]
fn a_const_object_records_entry_and_closing_spans_beside_a_nested_map() {
    let mut sub = SourceMapBuilder::new();
    sub.record(0, Span::new(41, 42));
    let mut raw = TsStmt::raw("    carried();\n", None);
    raw.nested_map = Some(sub);
    let entry =
        |name: &str, doc: Option<&str>, span: usize, body: Vec<TsStmt>| TsObjectEntry::Method {
            name: name.to_string(),
            is_async: false,
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            doc: doc.map(str::to_owned),
            inline: false,
            body,
            span: sp(span),
        };
    let init = TsExpr::multiline_object_ending_at(
        vec![
            entry("a", Some("Does a."), 10, vec![raw]),
            entry(
                "b",
                None,
                20,
                vec![TsStmt::return_stmt(Some(id("x")), sp(25))],
            ),
        ],
        Span::new(90, 91),
    );
    let decl = TsDecl::ConstDecl {
        name: "S".to_string(),
        ty: None,
        init,
    };
    let mut out = String::new();
    let mut map = SourceMapBuilder::new();
    print_stmt_and_merge(&mut out, &TsStmt::decl(decl, None), 0, &mut map, 0);
    let lines: Vec<(&str, usize)> = map
        .checkpoint_spans()
        .iter()
        .map(|(o, s)| (line_at(&out, *o), s.start))
        .collect();
    assert_eq!(
        lines,
        [
            ("  /**", 10),
            ("    carried();", 41),
            ("  b() {", 20),
            ("    return x;", 25),
            ("};", 90),
        ]
    );
}

/// `clear_spans` removes the span of a statement and of every statement nested in it, through
/// an `if`, a block, and an arrow's block body, so a printed map records nothing for it.
#[test]
fn clear_spans_clears_every_nested_statement() {
    let span = Span::new(0, 4);
    let inner = TsStmt::expr_stmt(TsExpr::Ident("x".to_string()), Some(span));
    let arrow = TsExpr::Arrow {
        params: Vec::new(),
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Block(vec![TsStmt::return_stmt(
            Some(TsExpr::Ident("y".to_string())),
            Some(span),
        )])),
    };
    let mut stmt = TsStmt::if_stmt(arrow, TsStmt::block(vec![inner], Some(span)), Some(span));
    assert!(!print_stmt_with_spans(&stmt, 0).1.is_empty());
    stmt.clear_spans();
    assert!(print_stmt_with_spans(&stmt, 0).1.is_empty());
}

/// A switch case's own span, a class method's span and a class's closing span are cleared too
/// (1B.5c: a stub class maps nothing).
#[test]
fn clear_spans_clears_case_method_and_class_end_spans() {
    let span = Span::new(0, 4);
    let case = TsSwitchCase {
        labels: vec![TsExpr::Lit(TsLit::Num("0".to_string()))],
        body: vec![TsStmt::return_stmt(None, None)],
        default_braced: false,
        case_braced: true,
        span: Some(span),
    };
    let switch = TsStmt::switch_stmt(TsExpr::Ident("k".to_string()), vec![case], None);
    let method = TsClassMethod {
        span: Some(span),
        ..TsClassMethod::new("m", false, Vec::new(), None, vec![switch])
    };
    let class = TsDecl::class("C", Vec::new(), None, vec![method]).ending_at(span);
    let mut stmt = TsStmt::decl(class, None);
    assert!(!print_stmt_with_spans(&stmt, 0).1.is_empty());
    stmt.clear_spans();
    assert!(print_stmt_with_spans(&stmt, 0).1.is_empty());
}

/// With no span set, no member or closing line is recorded, so every existing caller's map is
/// unchanged (1A.32a).
#[test]
fn a_class_and_a_const_object_without_spans_record_nothing() {
    let class = TsDecl::class(
        "C",
        Vec::new(),
        None,
        vec![TsClassMethod::new("a", false, Vec::new(), None, Vec::new())],
    );
    let object = TsDecl::ConstDecl {
        name: "S".to_string(),
        ty: None,
        init: TsExpr::multiline_object_entries(vec![TsObjectEntry::method(
            "a",
            false,
            Vec::new(),
            None,
            Vec::new(),
        )]),
    };
    for decl in [class, object] {
        let mut out = String::new();
        let mut map = SourceMapBuilder::new();
        print_stmt_and_merge(&mut out, &TsStmt::decl(decl, None), 0, &mut map, 0);
        assert!(map.checkpoint_spans().is_empty(), "{out}");
    }
}

/// 1A.25 (#3726, Decision C): `as` is left-associative, so a left-nested `As` operand needs no
/// brackets; the cast chain the cross-context boundary builds prints as `a as unknown as T`.
#[test]
fn a_left_nested_as_prints_without_brackets() {
    let cast = TsExpr::As {
        expr: Box::new(TsExpr::As {
            expr: Box::new(TsExpr::Ident("a".to_string())),
            ty: TsType::named("unknown"),
        }),
        ty: TsType::named("T"),
    };
    assert_eq!(print_expr(&cast), "a as unknown as T");
    assert_eq!(
        print_expr(&TsExpr::Paren(Box::new(cast))),
        "(a as unknown as T)"
    );
}

/// 1A.31 (#3732, Decision B): `AsConst` prints `<expr> as const`, bare inside an object property.
#[test]
fn an_as_const_prints_inside_an_object_property() {
    let tag = TsExpr::AsConst(Box::new(TsExpr::Lit(TsLit::Str("Some".to_string()))));
    let obj = TsExpr::Object {
        entries: vec![
            TsObjectEntry::prop("tag".to_string(), tag.clone()),
            TsObjectEntry::prop("value".to_string(), TsExpr::Ident("v".to_string())),
        ],
        multiline: false,
        end_span: None,
    };
    assert_eq!(print_expr(&tag), "\"Some\" as const");
    assert_eq!(print_expr(&obj), "{ tag: \"Some\" as const, value: v }");
}

/// `AsConst` as an arrow body: an object body is bracketed by the arrow, the tag inside keeps its
/// `as const`, and a conditional under `AsConst` is bracketed like it is under `As`.
#[test]
fn an_as_const_prints_inside_an_arrow_body() {
    let arrow = |body: TsExpr| TsExpr::Arrow {
        params: vec![],
        is_async: false,
        generics: vec![],
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(body))),
    };
    let tag = TsExpr::AsConst(Box::new(TsExpr::Lit(TsLit::Str("None".to_string()))));
    assert_eq!(print_expr(&arrow(tag)), "() => \"None\" as const");
    let obj = TsExpr::Object {
        entries: vec![TsObjectEntry::prop(
            "tag".to_string(),
            TsExpr::AsConst(Box::new(TsExpr::Lit(TsLit::Str("None".to_string())))),
        )],
        multiline: false,
        end_span: None,
    };
    assert_eq!(
        print_expr(&arrow(obj)),
        "() => ({ tag: \"None\" as const })"
    );
    let cond = TsExpr::AsConst(Box::new(TsExpr::Conditional {
        test: Box::new(TsExpr::Ident("c".to_string())),
        consequent: Box::new(TsExpr::Ident("a".to_string())),
        alternate: Box::new(TsExpr::Ident("b".to_string())),
    }));
    assert_eq!(print_expr(&cond), "(c ? a : b) as const");
}

/// `AsConst` is byte-identical to the `As { ty: const }` it replaces.
#[test]
fn an_as_const_matches_the_as_over_a_type_named_const() {
    let old = TsExpr::As {
        expr: Box::new(TsExpr::Lit(TsLit::Str("x".to_string()))),
        ty: TsType::named("const"),
    };
    let new = TsExpr::AsConst(Box::new(TsExpr::Lit(TsLit::Str("x".to_string()))));
    assert_eq!(print_expr(&old), print_expr(&new));
}

/// A cast that is the operand of a tighter-binding position (a member object, a call callee, an
/// `await`) is bracketed, and an explicit `Paren` under an `As` is kept: only the plain
/// left-nested chain is bracket-free.
#[test]
fn an_as_in_an_operand_position_keeps_its_brackets() {
    let inner = || TsExpr::As {
        expr: Box::new(TsExpr::Ident("a".to_string())),
        ty: TsType::named("T"),
    };
    assert_eq!(
        print_expr(&TsExpr::Member {
            object: Box::new(inner()),
            property: "x".to_string(),
        }),
        "(a as T).x"
    );
    assert_eq!(
        print_expr(&TsExpr::Call {
            callee: Box::new(inner()),
            args: vec![],
        }),
        "(a as T)()"
    );
    assert_eq!(
        print_expr(&TsExpr::As {
            expr: Box::new(TsExpr::Paren(Box::new(TsExpr::As {
                expr: Box::new(TsExpr::Ident("a".to_string())),
                ty: TsType::named("unknown"),
            }))),
            ty: TsType::named("T"),
        }),
        "(a as unknown) as T"
    );
}

// --- 1A.35a part B: labels, `while`, multi-declarator `let`, sequence, optional call,
// generators, `for await`, `++x` and array-pattern parameters ---

fn num(n: &str) -> TsExpr {
    TsExpr::Lit(TsLit::Num(n.to_string()))
}

fn lt(a: TsExpr, b: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: TsBinaryOp::LessThan,
        left: Box::new(a),
        right: Box::new(b),
    }
}

/// `while (cond) { … }`, braced as `for...of`'s body is, and inline for a bare statement.
#[test]
fn a_while_loop_prints_its_body_braced_or_inline() {
    let braced = TsStmt::while_stmt(
        lt(id("i"), num("3")),
        TsStmt::block(vec![TsStmt::increment(id("i"), None)], None),
        None,
    );
    assert_eq!(print_stmt(&braced, 0), "while (i < 3) {\n  i++;\n}\n");
    assert_eq!(print_stmt(&braced, 1), "  while (i < 3) {\n    i++;\n  }\n");
    let inline = TsStmt::while_stmt(id("go"), TsStmt::break_stmt(None), None);
    assert_eq!(print_stmt(&inline, 0), "while (go) break;\n");
}

/// A label continues its statement's line: `label: {` and `label: while (true) {`, and a
/// labelled `continue` / `break` names it.
#[test]
fn a_labelled_statement_and_its_jumps_print_on_the_label_line() {
    let block = TsStmt::labelled(
        "out",
        TsStmt::block(vec![TsStmt::break_label_stmt("out", None)], None),
        None,
    );
    assert_eq!(print_stmt(&block, 0), "out: {\n  break out;\n}\n");
    let looped = TsStmt::labelled(
        "LOOP",
        TsStmt::while_stmt(
            TsExpr::Lit(TsLit::Bool(true)),
            TsStmt::block(vec![TsStmt::continue_label_stmt("LOOP", None)], None),
            None,
        ),
        None,
    );
    assert_eq!(
        print_stmt(&looped, 1),
        "  LOOP: while (true) {\n    continue LOOP;\n  }\n"
    );
    // A bare `continue` / `break` still prints without a label.
    assert_eq!(print_stmt(&TsStmt::continue_stmt(None), 0), "continue;\n");
    assert_eq!(print_stmt(&TsStmt::break_stmt(None), 0), "break;\n");
}

/// A label on a statement inside an inline branch nests at the real depth.
#[test]
fn a_labelled_loop_inside_an_inline_block_keeps_its_depth() {
    let inner = TsStmt::labelled(
        "L",
        TsStmt::while_stmt(
            id("c"),
            TsStmt::block(vec![TsStmt::continue_label_stmt("L", None)], None),
            None,
        ),
        None,
    );
    let outer = TsStmt::if_stmt(id("a"), inner, None);
    assert_eq!(
        print_stmt(&outer, 1),
        "  if (a) L: while (c) {\n    continue L;\n  }\n"
    );
}

#[test]
fn a_let_list_prints_each_declarator() {
    let plain = TsStmt::let_list(vec![TsDeclarator::new("a"), TsDeclarator::new("b")], None);
    assert_eq!(print_stmt(&plain, 0), "let a, b;\n");
    let mixed = TsStmt::let_list(
        vec![
            TsDeclarator::new("a").typed(TsType::named("number")),
            TsDeclarator::new("b").definite(TsType::named("T")),
            TsDeclarator::new("c").init(num("1")),
        ],
        None,
    );
    assert_eq!(print_stmt(&mixed, 1), "  let a: number, b!: T, c = 1;\n");
}

/// A comma expression brackets itself, so it is safe as an argument, an arrow body and a
/// callee's operand.
#[test]
fn a_sequence_brackets_itself() {
    let seq = TsExpr::Sequence(vec![
        TsExpr::Call {
            callee: Box::new(TsExpr::Member {
                object: Box::new(id("s")),
                property: "delete".to_string(),
            }),
            args: vec![id("x")],
        },
        id("s"),
    ]);
    assert_eq!(print_expr(&seq), "(s.delete(x), s)");
    let call = TsExpr::Call {
        callee: Box::new(id("f")),
        args: vec![seq.clone(), id("y")],
    };
    assert_eq!(print_expr(&call), "f((s.delete(x), s), y)");
    let arrow = TsExpr::Arrow {
        params: vec![TsParam::new("s", None)],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(seq))),
    };
    assert_eq!(print_expr(&arrow), "(s) => (s.delete(x), s)");
}

#[test]
fn an_optional_call_prints_question_dot_parens() {
    let member = TsExpr::Member {
        object: Box::new(id("o")),
        property: "track".to_string(),
    };
    let call = TsExpr::OptionalCall {
        callee: Box::new(member),
        args: vec![],
    };
    assert_eq!(print_expr(&call), "o.track?.()");
    // A member of an optional call is a plain member access, and `new` brackets it.
    let chained = TsExpr::Member {
        object: Box::new(call.clone()),
        property: "x".to_string(),
    };
    assert_eq!(print_expr(&chained), "o.track?.().x");
    let new = TsExpr::New {
        callee: Box::new(chained),
        args: vec![],
    };
    assert_eq!(print_expr(&new), "new (o.track?.().x)()");
    // A looser callee is bracketed.
    let sum = TsExpr::Binary {
        op: TsBinaryOp::Or,
        left: Box::new(id("a")),
        right: Box::new(id("b")),
    };
    let looser = TsExpr::OptionalCall {
        callee: Box::new(sum),
        args: vec![num("1")],
    };
    assert_eq!(print_expr(&looser), "(a || b)?.(1)");
}

/// A generator function expression prints its body one level deeper, `yield` as an
/// expression statement, `for await` over an async iterable, and brackets as a callee.
#[test]
fn an_async_generator_with_yield_and_for_await_prints_as_an_iife() {
    let generator = TsExpr::Function {
        is_async: true,
        is_generator: true,
        params: vec![TsParam::new("__s", None)],
        return_type: None,
        body: vec![TsStmt::for_await_of(
            "__e",
            id("__s"),
            TsStmt::block(
                vec![TsStmt::expr_stmt(TsExpr::Yield(Box::new(id("__e"))), None)],
                None,
            ),
            None,
        )],
    };
    let call = TsExpr::Call {
        callee: Box::new(generator),
        args: vec![id("src")],
    };
    assert_eq!(
        print_stmt(&TsStmt::expr_stmt(call.clone(), None), 0),
        "(async function* (__s) {\n  for await (const __e of __s) {\n    yield __e;\n  }\n})(src);\n"
    );
    assert_eq!(
        print_stmt(&TsStmt::expr_stmt(call, None), 1),
        "  (async function* (__s) {\n    for await (const __e of __s) {\n      yield __e;\n    }\n  })(src);\n"
    );
}

#[test]
fn a_plain_function_expression_and_a_bracketed_yield() {
    let function = TsExpr::Function {
        is_async: false,
        is_generator: false,
        params: vec![TsParam::new("a", Some(TsType::named("number")))],
        return_type: Some(TsType::named("number")),
        body: vec![TsStmt::return_stmt(Some(id("a")), None)],
    };
    assert_eq!(
        print_expr(&function),
        "function (a: number): number {\n  return a;\n}"
    );
    // `yield` binds looser than every operator, so it is bracketed as an operand.
    let yielded = TsExpr::Yield(Box::new(id("v")));
    let call = TsExpr::Call {
        callee: Box::new(id("f")),
        args: vec![yielded.clone()],
    };
    assert_eq!(print_expr(&call), "f(yield v)");
    let sum = TsExpr::Binary {
        op: TsBinaryOp::Add,
        left: Box::new(yielded.clone()),
        right: Box::new(num("1")),
    };
    assert_eq!(print_expr(&sum), "(yield v) + 1");
    let cast = TsExpr::As {
        expr: Box::new(yielded),
        ty: TsType::named("T"),
    };
    assert_eq!(print_expr(&cast), "(yield v) as T");
}

#[test]
fn a_prefix_increment_prints_before_its_operand() {
    let inc = TsExpr::Unary {
        op: TsUnaryOp::PreIncrement,
        expr: Box::new(id("__i")),
    };
    let test = TsExpr::Binary {
        op: TsBinaryOp::GreaterThanEq,
        left: Box::new(inc),
        right: Box::new(id("__n")),
    };
    assert_eq!(print_expr(&test), "++__i >= __n");
}

#[test]
fn an_array_pattern_parameter_prints_in_place_of_its_name() {
    let arrow = TsExpr::Arrow {
        params: vec![TsParam::array_pattern(
            vec!["__k".to_string(), "__v".to_string()],
            None,
        )],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(id("__v")))),
    };
    assert_eq!(print_expr(&arrow), "([__k, __v]) => __v");
    let typed = TsParam::array_pattern(
        vec![String::new(), "b".to_string()],
        Some(TsType::named("T")),
    );
    let mut out = String::new();
    render_params(&mut out, &[typed, TsParam::new("c", None)]);
    assert_eq!(out, "[, b]: T, c");
}

/// `for await` awaits, whatever its body does; a `while`, a label and a `let` list are
/// walked for an `await` in their expressions.
#[test]
fn the_new_statements_are_walked_for_awaits() {
    use crate::expr_contains_await;
    let awaiting = || TsExpr::Await(Box::new(id("p")));
    let for_await = TsStmt::for_await_of("e", id("s"), TsStmt::block(vec![], None), None);
    let wrap = |stmt: TsStmt| TsExpr::Function {
        is_async: true,
        is_generator: false,
        params: vec![],
        return_type: None,
        body: vec![stmt],
    };
    assert!(expr_contains_await(&wrap(for_await)));
    let in_cond = TsStmt::while_stmt(awaiting(), TsStmt::block(vec![], None), None);
    assert!(expr_contains_await(&wrap(in_cond)));
    let in_label = TsStmt::labelled(
        "L",
        TsStmt::while_stmt(id("c"), TsStmt::expr_stmt(awaiting(), None), None),
        None,
    );
    assert!(expr_contains_await(&wrap(in_label)));
    let in_let = TsStmt::let_list(vec![TsDeclarator::new("a").init(awaiting())], None);
    assert!(expr_contains_await(&wrap(in_let)));
    assert!(!expr_contains_await(&wrap(TsStmt::let_list(
        vec![TsDeclarator::new("a")],
        None
    ))));
    assert!(expr_contains_await(&TsExpr::Sequence(vec![
        id("a"),
        awaiting()
    ])));
    assert!(expr_contains_await(&TsExpr::OptionalCall {
        callee: Box::new(id("f")),
        args: vec![awaiting()],
    }));
    assert!(expr_contains_await(&TsExpr::Yield(Box::new(awaiting()))));
}

/// The statements of a labelled loop record their checkpoints exactly as they would in an
/// unlabelled one (1A.35a part B): the label does not hide them from the source map.
#[test]
fn a_labelled_loops_statements_record_their_spans() {
    let inner = TsStmt::return_stmt(Some(id("x")), sp(7));
    let looped = TsStmt::labelled(
        "L",
        TsStmt::while_stmt(id("c"), TsStmt::block(vec![inner.clone()], None), None),
        sp(1),
    );
    let (text, spans) = print_stmt_with_spans(&looped, 0);
    assert_eq!(text, "L: while (c) {\n  return x;\n}\n");
    assert_eq!(
        spans,
        vec![
            (0, Span::new(1, 2)),
            (text.find("  return").unwrap(), Span::new(7, 8))
        ]
    );
    // The same loop unlabelled records the same inner checkpoint.
    let bare = TsStmt::while_stmt(id("c"), TsStmt::block(vec![inner], None), None);
    let (text, spans) = print_stmt_with_spans(&bare, 0);
    assert_eq!(
        spans,
        vec![(text.find("  return").unwrap(), Span::new(7, 8))]
    );
}

/// A `FormFieldSpecs` carrier prints its source text exactly, in argument position (1A.35a
/// part C): `__formDecode(__s, [{ name: "a", kind: "string", list: false, option: false }])`.
#[test]
fn a_form_field_specs_carrier_prints_its_text_verbatim_as_an_argument() {
    let specs = "[{ name: \"a\", kind: \"string\", list: false, option: false }]";
    let call = TsExpr::Call {
        callee: Box::new(id("__formDecode")),
        args: vec![
            id("__s"),
            TsExpr::VerbatimExpr(specs.to_string(), VerbatimOrigin::FormFieldSpecs),
        ],
    };
    assert_eq!(print_expr(&call), format!("__formDecode(__s, {specs})"));
}

/// 1A.38a: a default import that also names braced imports, the Worker group's
/// member import. The names keep `TsDecl::Import`'s `a as b` spelling.
#[test]
fn prints_a_default_import_with_named_imports() {
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ImportDefault {
            alias: "__entry0".to_string(),
            named: vec!["isMiss as __miss0".to_string(), "type T".to_string()],
            from: "./shop.index.js".to_string(),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "import __entry0, { isMiss as __miss0, type T } from \"./shop.index.js\";\n"
    );
}

/// 1A.38a: the multiline `Conditional` is the test, then `? a` and `: b` one level
/// deeper than the statement holding it, here a depth-1 `const` in a function
/// body. Its three sub-expressions print as they do in a one-line `Conditional`.
#[test]
fn prints_a_multiline_conditional_one_level_deeper_than_its_statement() {
    let cond = TsExpr::multiline_conditional(
        TsExpr::Paren(Box::new(TsExpr::Binary {
            op: TsBinaryOp::StrictEq,
            left: Box::new(id("a")),
            right: Box::new(id("b")),
        })),
        TsExpr::Call {
            callee: Box::new(id("f")),
            args: vec![id("x")],
        },
        id("y"),
    );
    let decl = TsDecl::Function {
        name: "g".to_string(),
        generics: vec![],
        params: vec![],
        return_type: None,
        body: vec![TsStmt::const_stmt(
            TsBindingName::Ident("v".to_string()),
            None,
            cond,
            None,
        )],
        is_async: false,
        inline: false,
    };
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(decl, None));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "function g() {\n  const v = (a === b)\n    ? f(x)\n    : y;\n}\n"
    );
}

/// 1A.38a: a `MultilineConditional` takes `Conditional`'s precedence rules: it is
/// bracketed as a binary operand, as a call callee, and as another conditional's
/// test.
#[test]
fn a_multiline_conditional_is_bracketed_wherever_a_conditional_is() {
    let cond = || TsExpr::multiline_conditional(id("t"), id("c"), id("a"));
    let one_line = || TsExpr::Conditional {
        test: Box::new(id("t")),
        consequent: Box::new(id("c")),
        alternate: Box::new(id("a")),
    };
    let wrapped = |e: TsExpr| TsExpr::Binary {
        op: TsBinaryOp::Add,
        left: Box::new(e),
        right: Box::new(id("z")),
    };
    assert_eq!(
        print_expr(&wrapped(cond())),
        "(t\n  ? c\n  : a) + z".to_string()
    );
    assert_eq!(
        print_expr(&TsExpr::Call {
            callee: Box::new(cond()),
            args: vec![]
        }),
        "(t\n  ? c\n  : a)()"
    );
    assert_eq!(
        print_expr(&TsExpr::multiline_conditional(one_line(), id("c"), id("a"))),
        "(t ? c : a)\n  ? c\n  : a"
    );
    assert_eq!(
        print_expr(&TsExpr::multiline_conditional(cond(), id("c"), id("a"))),
        "(t\n  ? c\n  : a)\n  ? c\n  : a"
    );
}

/// 1A.38a: an `else if` continues the chain at the depth of the first `if`, so the
/// blocks of every link nest where the first one's do, however deep the chain sits.
#[test]
fn an_else_if_chain_nests_each_link_at_the_chains_own_depth() {
    let guard = |name: &str| {
        TsStmt::if_stmt(
            id(name),
            TsStmt::block(vec![TsStmt::return_stmt(Some(id("x")), None)], None),
            None,
        )
    };
    let link = |name: &str, then: TsStmt, rest: TsStmt| {
        TsStmt::if_else_same_line_stmt(
            id(name),
            TsStmt::block(vec![then, TsStmt::return_stmt(Some(id("y")), None)], None),
            rest,
            None,
        )
    };
    let chain = link(
        "a",
        guard("g"),
        TsStmt::if_stmt(id("b"), TsStmt::block(vec![guard("h")], None), None),
    );
    let decl = TsDecl::Function {
        name: "f".to_string(),
        generics: vec![],
        params: vec![],
        return_type: None,
        body: vec![chain],
        is_async: false,
        inline: false,
    };
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(decl, None));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "function f() {\n  if (a) {\n    if (g) {\n      return x;\n    }\n    return y;\n  } else if (b) {\n    if (h) {\n      return x;\n    }\n  }\n}\n"
    );
}

/// 1A.38a review: an `else if` chain **without** `same_line_else` takes the fresh-line
/// path, so each `else if` starts its own line at the chain's depth and its blocks nest
/// where the first `if`'s do.
#[test]
fn a_fresh_line_else_if_chain_nests_each_link_at_the_chains_own_depth() {
    let guard = |name: &str| {
        TsStmt::if_stmt(
            id(name),
            TsStmt::block(vec![TsStmt::return_stmt(Some(id("x")), None)], None),
            None,
        )
    };
    let chain = TsStmt::if_else_stmt(
        id("a"),
        TsStmt::block(vec![guard("g")], None),
        TsStmt::if_else_stmt(
            id("b"),
            TsStmt::block(vec![guard("h")], None),
            TsStmt::block(vec![guard("i")], None),
            None,
        ),
        None,
    );
    let decl = TsDecl::Function {
        name: "f".to_string(),
        generics: vec![],
        params: vec![],
        return_type: None,
        body: vec![chain],
        is_async: false,
        inline: false,
    };
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(decl, None));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "function f() {\n  if (a) {\n    if (g) {\n      return x;\n    }\n  }\n  else if (b) {\n    if (h) {\n      return x;\n    }\n  }\n  else {\n    if (i) {\n      return x;\n    }\n  }\n}\n"
    );
}

/// 1A.38a review: a chain of three or more links (`if`, two `else if`s and a final
/// `else`) keeps every link at the first `if`'s depth, not one level deeper per link.
#[test]
fn a_four_link_else_if_chain_keeps_every_link_at_one_depth() {
    let body = |name: &str| TsStmt::block(vec![TsStmt::return_stmt(Some(id(name)), None)], None);
    let chain = TsStmt::if_else_same_line_stmt(
        id("a"),
        body("w"),
        TsStmt::if_else_same_line_stmt(
            id("b"),
            body("x"),
            TsStmt::if_else_same_line_stmt(id("c"), body("y"), body("z"), None),
            None,
        ),
        None,
    );
    let decl = TsDecl::Function {
        name: "f".to_string(),
        generics: vec![],
        params: vec![],
        return_type: None,
        body: vec![TsStmt::if_stmt(
            id("outer"),
            TsStmt::block(vec![chain], None),
            None,
        )],
        is_async: false,
        inline: false,
    };
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(decl, None));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "function f() {\n  if (outer) {\n    if (a) {\n      return w;\n    } else if (b) {\n      return x;\n    } else if (c) {\n      return y;\n    } else {\n      return z;\n    }\n  }\n}\n"
    );
}

/// 1A.38b: a multiline object under `as const` or `as T` keeps its layout, in a
/// declaration's initialiser and in a `return`, with an arrow-valued property whose
/// body holds a block arrow nested inside it.
#[test]
fn a_cast_multiline_object_keeps_its_layout_in_a_const_and_a_return() {
    let id = |n: &str| TsExpr::Ident(n.to_string());
    let route = TsExpr::Arrow {
        params: vec![TsParam::new("init", None)],
        is_async: false,
        generics: vec![],
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::As {
            expr: Box::new(TsExpr::Call {
                callee: Box::new(id("__call")),
                args: vec![
                    id("options"),
                    TsExpr::Arrow {
                        params: vec![],
                        is_async: false,
                        generics: vec![],
                        return_type: None,
                        body: Box::new(TsArrowBody::Multiline(vec![TsStmt::return_stmt(
                            Some(id("x")),
                            None,
                        )])),
                    },
                ],
            }),
            ty: TsType::named("T"),
        }))),
    };
    let object = || {
        TsExpr::multiline_object(vec![(
            "svc".to_string(),
            TsExpr::multiline_object(vec![
                ("a".to_string(), TsExpr::Lit(TsLit::Str("x".to_string()))),
                ("route".to_string(), route.clone()),
            ]),
        )])
    };
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::ConstDecl {
            name: "API".to_string(),
            ty: None,
            init: TsExpr::AsConst(Box::new(object())),
        })),
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::Function {
            name: "f".to_string(),
            generics: vec![],
            params: vec![],
            return_type: None,
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::As {
                    expr: Box::new(object()),
                    ty: TsType::named("T"),
                }),
                None,
            )],
            is_async: false,
            inline: false,
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "export const API = {\n  svc: {\n    a: \"x\",\n    route: (init) => __call(options, () => {\n      return x;\n    }) as T,\n  },\n} as const;\n\nfunction f() {\n  return {\n    svc: {\n      a: \"x\",\n      route: (init) => __call(options, () => {\n        return x;\n      }) as T,\n    },\n  } as T;\n}\n"
    );
}

/// 1A.38b: a multiline array under `as const` breaks one item per line too, and a
/// one-line object under a cast is untouched.
#[test]
fn a_cast_multiline_array_keeps_its_layout_and_a_one_line_object_does_not_change() {
    let item = |n: &str| TsExpr::Lit(TsLit::Str(n.to_string()));
    let mut program = TsProgram::new();
    program.push(TsStmt::decl(
        TsDecl::ConstDecl {
            name: "xs".to_string(),
            ty: None,
            init: TsExpr::AsConst(Box::new(TsExpr::multiline_array(vec![
                item("a"),
                item("b"),
            ]))),
        },
        None,
    ));
    program.push(TsStmt::decl(
        TsDecl::ConstDecl {
            name: "o".to_string(),
            ty: None,
            init: TsExpr::AsConst(Box::new(TsExpr::object(vec![("k".to_string(), item("v"))]))),
        },
        None,
    ));
    let printed = print(&program, "x.bynk", "", "x.ts");
    assert_eq!(
        printed.text,
        "const xs = [\n  \"a\",\n  \"b\",\n] as const;\n\nconst o = { k: \"v\" } as const;\n"
    );
}

/// 1A.38b: a parameter's default prints after its type, `query: Q = {}`.
#[test]
fn a_parameter_default_prints_after_the_type() {
    let mut out = String::new();
    render_params(
        &mut out,
        &[
            TsParam::new("options", Some(TsType::named("ClientOptions")))
                .with_default(TsExpr::object(vec![])),
            TsParam::new("path", Some(TsType::named("string")))
                .with_default(TsExpr::Lit(TsLit::Str("$".to_string()))),
        ],
    );
    assert_eq!(out, "options: ClientOptions = {}, path: string = \"$\"");
}

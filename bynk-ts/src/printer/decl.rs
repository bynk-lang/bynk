use super::compact::{inline_block_fits_one_line, render_inline_block};
use super::expr::render_expr;
use super::fragment::contains_raw;
use super::multiline::{render_bare_generics, render_stmt_level_expr};
use super::stmt::{render_block_stmts, render_doc_comment};
use super::types::{render_type, render_type_member};
use super::{MergeTarget, indent, note_span_at};
use crate::program::{TsClassMethod, TsDecl, TsParam, TsType, TsTypeMember};

/// The one real per-method render, shared by [`print_class_method`](super::fragment::print_class_method) and
/// [`TsDecl::Class`]'s own methods-loop arm — review of #1360, finding 4:
/// keeping two independent copies of this shape was exactly the drift risk
/// this track exists to design against (two printers disagreeing about a
/// formatting convention). `depth` means the *class's* own depth, matching
/// both callers' own convention.
///
/// Review of #1360, finding 1: a `TsStmtKind::Raw` method body has no
/// indent of its own — its text is captured pre-indented at a fixed
/// absolute depth by its own caller — so it only renders correctly when
/// `depth` is `0`, the same hazard `render_multiline_object_entry`'s own
/// identical `debug_assert!` guards (review of #1338 finding 3, relocated
/// by review of #1340 finding 1). `print_class_method`'s own doc frames it
/// as a general fragment entry point, so a future caller at a non-zero
/// depth is the expected case this guards against, not a hypothetical.
///
/// `map`: see [`render_block_stmts`]'s own doc (#1477) — forwarded
/// unchanged to `method.body`'s own rendering.
pub(super) fn render_class_method(
    out: &mut String,
    method: &TsClassMethod,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    debug_assert!(
        depth == 0 || !contains_raw(&method.body),
        "render_class_method: a Raw method body's own baked-in indent only matches depth 0"
    );
    if let Some(span) = method.span {
        note_span_at(out.len(), span, map.as_mut());
    }
    if let Some(text) = &method.doc {
        render_doc_comment(out, text, depth + 1);
    }
    out.push_str(&indent(depth + 1));
    if method.private {
        out.push_str("private ");
    }
    if method.is_async {
        out.push_str("async ");
    }
    out.push_str(&method.name);
    out.push('(');
    render_params(out, &method.params);
    out.push(')');
    if let Some(rt) = &method.return_type {
        out.push_str(": ");
        render_type(out, rt);
    }
    render_block_stmts(out, &method.body, depth + 1, map);
    out.push('\n');
}

/// Whether `ty` is `undefined` or a union with an `undefined` member: the
/// shape of an optional field, which a definite-assignment assertion
/// contradicts.
fn type_admits_undefined(ty: &TsType) -> bool {
    match ty {
        TsType::Named { name, type_args } => name == "undefined" && type_args.is_empty(),
        TsType::Union { members, .. } => members.iter().any(type_admits_undefined),
        TsType::Array { .. }
        | TsType::Object(_)
        | TsType::Fn { .. }
        | TsType::Intersection(_)
        | TsType::Mapped { .. }
        | TsType::TypeOf(_)
        | TsType::Indexed { .. }
        | TsType::StringLiteral(_)
        | TsType::NumberLiteral(_) => false,
        TsType::Paren(inner) => type_admits_undefined(inner),
    }
}

pub(super) fn render_params(out: &mut String, params: &[TsParam]) {
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        debug_assert!(
            !(p.rest && p.optional),
            "render_params: a rest parameter cannot be optional (`...{}?` is not valid TypeScript)",
            p.name
        );
        if p.rest {
            out.push_str("...");
        }
        match &p.array_pattern {
            Some(names) => {
                out.push('[');
                out.push_str(&names.join(", "));
                out.push(']');
            }
            None => out.push_str(&p.name),
        }
        debug_assert!(
            !(p.optional && p.default.is_some()),
            "render_params: a parameter cannot be optional and have a default (`{}?: T = v` is not valid TypeScript)",
            p.name
        );
        if p.optional {
            out.push('?');
        }
        if let Some(ty) = &p.ty {
            out.push_str(": ");
            render_type(out, ty);
        }
        if let Some(default) = &p.default {
            out.push_str(" = ");
            render_expr(out, default, 0, &mut None);
        }
    }
}

pub(super) fn render_decl(
    out: &mut String,
    decl: &TsDecl,
    depth: usize,
    map: Option<MergeTarget<'_>>,
) {
    // `render_decl_body`'s own `TsDecl::Export` arm already writes
    // `"export "` before recursing into the inner declaration — the
    // `if let Export` special case this replaced produced byte-identical
    // output by duplicating that same logic one layer up (review of
    // #1314, smaller note: the two branches were provably the same for
    // every input, so only one needs to exist).
    out.push_str(&indent(depth));
    render_decl_body(out, decl, depth, map);
}

/// The declaration's own text, with no leading indent — [`render_decl`]
/// writes the indent (and, for `Export`, the `export ` keyword) once, then
/// hands off here so `Export(inner)` doesn't duplicate `inner`'s own
/// leading whitespace.
///
/// `map`: see [`render_block_stmts`]'s own doc (#1477) — reaches a
/// `TsDecl::Function`'s own body, and a `TsDecl::Class`'s own constructor/
/// methods, unchanged; every other arm ignores it (no body of its own to
/// carry a `nested_map`).
pub(super) fn render_decl_body(
    out: &mut String,
    decl: &TsDecl,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    match decl {
        TsDecl::Import {
            type_only,
            names,
            from,
        } => {
            out.push_str(if *type_only {
                "import type { "
            } else {
                "import { "
            });
            out.push_str(&names.join(", "));
            out.push_str(" } from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::ImportNamespace {
            type_only,
            alias,
            from,
        } => {
            out.push_str(if *type_only {
                "import type * as "
            } else {
                "import * as "
            });
            out.push_str(alias);
            out.push_str(" from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::ImportDefault { alias, named, from } => {
            out.push_str("import ");
            out.push_str(alias);
            if !named.is_empty() {
                out.push_str(", { ");
                out.push_str(&named.join(", "));
                out.push_str(" }");
            }
            out.push_str(" from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::ReExport { names, from } => {
            out.push_str("export { ");
            out.push_str(&names.join(", "));
            out.push_str(" } from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::ExportNames { names } => {
            out.push_str("export { ");
            out.push_str(&names.join(", "));
            out.push_str(" };\n");
        }
        TsDecl::ReExportAll { from } => {
            out.push_str("export * from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::Export(inner) => {
            out.push_str("export ");
            render_decl_body(out, inner, depth, map);
        }
        TsDecl::Interface {
            name,
            type_params,
            members,
        } => {
            out.push_str("interface ");
            out.push_str(name);
            render_bare_generics(out, type_params);
            out.push_str(" {\n");
            for member in members {
                // #1357: a `Method` member's own `doc` renders here, not in
                // `render_type_member` itself — that function has no
                // `depth` to give `render_doc_comment`, the same "doc lives
                // at the call site that carries depth" split
                // `render_multiline_object_entry`'s own `Method` arm
                // already uses for the identical field.
                if let TsTypeMember::Method {
                    doc: Some(text), ..
                } = member
                {
                    render_doc_comment(out, text, depth + 1);
                }
                out.push_str(&indent(depth + 1));
                render_type_member(out, member);
                out.push_str(";\n");
            }
            out.push_str(&indent(depth));
            out.push_str("}\n");
        }
        TsDecl::ConstDecl { name, ty, init } => {
            out.push_str("const ");
            out.push_str(name);
            if let Some(ty) = ty {
                out.push_str(": ");
                render_type(out, ty);
            }
            out.push_str(" = ");
            render_stmt_level_expr(out, init, depth, &mut map);
            out.push_str(";\n");
        }
        TsDecl::Class {
            name,
            implements,
            fields,
            constructor,
            methods,
            end_span,
        } => {
            out.push_str("class ");
            out.push_str(name);
            for (i, ty) in implements.iter().enumerate() {
                out.push_str(if i == 0 { " implements " } else { ", " });
                render_type(out, ty);
            }
            out.push_str(" {\n");
            for f in fields {
                debug_assert!(
                    !(f.definite && f.ty.as_ref().is_none_or(type_admits_undefined)),
                    "render_decl_body: field `{}` is definite (`!`) yet it has no type or its type admits `undefined`; \
                     an optional-shaped field needs no assertion",
                    f.name
                );
                out.push_str(&indent(depth + 1));
                if f.private {
                    out.push_str("private ");
                }
                out.push_str(&f.name);
                if f.definite {
                    out.push('!');
                }
                if let Some(ty) = &f.ty {
                    out.push_str(": ");
                    render_type(out, ty);
                }
                if let Some(init) = &f.init {
                    out.push_str(" = ");
                    render_expr(out, init, 0, &mut None);
                }
                out.push_str(";\n");
            }
            // Readability policy (this module's own doc): no blank line
            // between fields and the constructor; one blank line before
            // each method — `events_fanout.rs`'s own real class spacing.
            let mut wrote_member = !fields.is_empty();
            if let Some(ctor) = constructor {
                out.push_str(&indent(depth + 1));
                out.push_str("constructor(");
                render_params(out, &ctor.params);
                out.push(')');
                render_block_stmts(
                    out,
                    &ctor.body,
                    depth + 1,
                    map.as_mut().map(|t| t.reborrow()),
                );
                out.push('\n');
                wrote_member = true;
            }
            for m in methods {
                if wrote_member {
                    out.push('\n');
                }
                render_class_method(out, m, depth, map.as_mut().map(|t| t.reborrow()));
                wrote_member = true;
            }
            // 1A.32a: the closing line belongs to the class, not to the last
            // method's final body line.
            if let Some(span) = end_span {
                note_span_at(out.len(), *span, map.as_mut());
            }
            out.push_str(&indent(depth));
            out.push_str("}\n");
        }
        TsDecl::Function {
            name,
            generics,
            params,
            return_type,
            body,
            is_async,
            inline,
        } => {
            if *is_async {
                out.push_str("async ");
            }
            out.push_str("function ");
            out.push_str(name);
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push(')');
            if let Some(rt) = return_type {
                out.push_str(": ");
                render_type(out, rt);
            }
            // `inline` asks for the one-line body; a statement that prints over several lines
            // takes the ordinary multi-line one instead (1A.39a, Decision E).
            if *inline && inline_block_fits_one_line(body) {
                out.push(' ');
                render_inline_block(out, body);
            } else {
                render_block_stmts(out, body, depth, map);
                out.push('\n');
            }
        }
        TsDecl::TypeAlias {
            name,
            type_params,
            ty,
        } => {
            out.push_str("type ");
            out.push_str(name);
            render_bare_generics(out, type_params);
            // #1339: a multiline Union's own first rendered character is
            // its first member's leading indent, not a newline — the `=`
            // itself must be followed directly by `\n` here (no space
            // before it), matching the pre-conversion `writeln!(out,
            // "export type {name}{params} =")` line's own exact bytes;
            // every other `ty` keeps the ordinary `" = "` (space both
            // sides) ordinary single-line form.
            if matches!(
                ty,
                TsType::Union {
                    multiline: true,
                    ..
                }
            ) {
                out.push_str(" =\n");
            } else {
                out.push_str(" = ");
            }
            render_type(out, ty);
            out.push_str(";\n");
        }
        TsDecl::ExportDefault(expr) => {
            out.push_str("export default ");
            render_stmt_level_expr(out, expr, depth, &mut None);
            out.push_str(";\n");
        }
        TsDecl::DeclareConst { name, ty } => {
            out.push_str("declare const ");
            out.push_str(name);
            out.push_str(": ");
            render_type(out, ty);
            out.push_str(";\n");
        }
    }
}

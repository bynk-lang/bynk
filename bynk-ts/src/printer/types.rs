use super::decl::render_params;
use super::expr::{render_expr, render_lit};
use super::multiline::render_bare_generics;
use crate::program::{TsExpr, TsLit, TsMappedModifier, TsType, TsTypeMember};

pub(super) fn render_type(out: &mut String, ty: &TsType) {
    match ty {
        TsType::Named { name, type_args } => {
            out.push_str(name);
            if !type_args.is_empty() {
                out.push('<');
                for (i, arg) in type_args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    render_type(out, arg);
                }
                out.push('>');
            }
        }
        TsType::Array { element, readonly } => {
            if *readonly {
                out.push_str("readonly ");
            }
            if needs_parens_as_array_element(element) {
                out.push('(');
                render_type(out, element);
                out.push(')');
            } else {
                render_type(out, element);
            }
            out.push_str("[]");
        }
        TsType::Object(members) => {
            // Review of #1358, finding 1: a `Method` member's own `doc` has
            // no single-line form — this inline shape has nowhere to put a
            // JSDoc block, unlike `TsDecl::Interface`'s own render arm
            // (which calls `render_doc_comment` before a documented
            // member's own line, since it has `depth` and a real newline
            // budget to work with). The identical case #1338's own review
            // already ruled on for `TsObjectEntry::Method.doc` in
            // `render_object_entry_inline` — a loud check, not a silent
            // drop, since silently dropping a JSDoc block is a strictly
            // worse failure mode than a loud one. Not reachable today
            // (`workers_entry.rs`'s own one real `TsType::Object` method
            // member goes through `TsTypeMember::method`, so `doc` is
            // always `None`) — the #1337 case was equally unreachable,
            // which is exactly why it got the assert.
            debug_assert!(
                !members
                    .iter()
                    .any(|m| matches!(m, TsTypeMember::Method { doc: Some(_), .. })),
                "render_type: a Method member's own doc comment has no single-line form"
            );
            if members.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{ ");
                for (i, m) in members.iter().enumerate() {
                    if i > 0 {
                        out.push_str("; ");
                    }
                    render_type_member(out, m);
                }
                out.push_str(" }");
            }
        }
        TsType::Fn {
            params,
            names,
            rest,
            ret,
        } => {
            out.push('(');
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if *rest && i + 1 == params.len() {
                    out.push_str("...");
                }
                match names.get(i) {
                    Some(name) => out.push_str(name),
                    None => {
                        out.push('a');
                        out.push_str(&i.to_string());
                    }
                }
                out.push_str(": ");
                render_type(out, p);
            }
            out.push_str(") => ");
            render_type(out, ret);
        }
        TsType::Union {
            members,
            multiline: false,
        } => {
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                render_type(out, m);
            }
        }
        // #1339: `emit_sum_type`'s own real shape — one variant per line, a
        // leading `|` on every line except the first (which gets equivalent
        // spacing instead, matching the pre-conversion `writeln!` code's own
        // `let pipe = if i == 0 { " " } else { "|" };` exactly), the closing
        // `;` appended by `TsDecl::TypeAlias`'s own caller directly after
        // the last member's own line — see `TsType::Union`'s own doc for
        // why this needs no depth parameter despite the multi-line shape.
        TsType::Union {
            members,
            multiline: true,
        } => {
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str("  ");
                out.push_str(if i == 0 { " " } else { "|" });
                out.push(' ');
                render_type(out, m);
            }
        }
        TsType::Intersection(members) => {
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    out.push_str(" & ");
                }
                render_type(out, m);
            }
        }
        TsType::Mapped {
            readonly,
            key,
            keys_of,
            value,
        } => render_mapped_type(out, *readonly, key, keys_of, value),
        TsType::TypeOf(operand) => render_type_of(out, operand),
        TsType::Indexed { object, index } => render_indexed_type(out, object, index),
        TsType::StringLiteral(text) => render_lit(out, &TsLit::Str(text.clone())),
        TsType::NumberLiteral(n) => out.push_str(&n.to_string()),
        TsType::Paren(inner) => {
            out.push('(');
            render_type(out, inner);
            out.push(')');
        }
    }
}

/// `{ -readonly [K in keyof T]: V }`. A `keyof` operand that is itself a union,
/// an intersection or a function type is parenthesised, since `keyof` binds
/// tighter than each of them.
fn render_mapped_type(
    out: &mut String,
    readonly: Option<TsMappedModifier>,
    key: &str,
    keys_of: &TsType,
    value: &TsType,
) {
    out.push_str("{ ");
    match readonly {
        Some(TsMappedModifier::Add) => out.push_str("+readonly "),
        Some(TsMappedModifier::Remove) => out.push_str("-readonly "),
        None => {}
    }
    out.push('[');
    out.push_str(key);
    out.push_str(" in keyof ");
    render_type_parenthesised(
        out,
        keys_of,
        matches!(
            keys_of,
            TsType::Union { .. } | TsType::Intersection(_) | TsType::Fn { .. }
        ),
    );
    out.push_str("]: ");
    render_type(out, value);
    out.push_str(" }");
}

/// `typeof x`: the operand is a value expression, printed at depth 0 since an
/// identifier or member chain has no layout.
fn render_type_of(out: &mut String, operand: &TsExpr) {
    out.push_str("typeof ");
    render_expr(out, operand, 0, &mut None);
}

/// `T[K]`. An object that is a union, an intersection, a function type or a
/// `typeof` is parenthesised (`(typeof s)["k"]`), since the bare postfix would
/// bind to its last operand.
fn render_indexed_type(out: &mut String, object: &TsType, index: &TsType) {
    render_type_parenthesised(
        out,
        object,
        matches!(
            object,
            TsType::Union { .. }
                | TsType::Intersection(_)
                | TsType::Fn { .. }
                | TsType::TypeOf(_)
                | TsType::Array { readonly: true, .. }
        ),
    );
    out.push('[');
    render_type(out, index);
    out.push(']');
}

fn render_type_parenthesised(out: &mut String, ty: &TsType, parenthesise: bool) {
    if parenthesise {
        out.push('(');
        render_type(out, ty);
        out.push(')');
    } else {
        render_type(out, ty);
    }
}

/// Whether `element` must be parenthesised before a postfix `[]`. `[]` binds
/// tighter than every prefix or infix type operator, so a readonly array
/// (`readonly T[][]` reads as `readonly (T[][])`, which tsc rejects with
/// TS1354), a function type (`() => T[]` returns the array) and a union or
/// intersection (`A | B[]`) would each be misread bare. A mutable array, a
/// named type and an object literal are postfix-safe as they are.
fn needs_parens_as_array_element(element: &TsType) -> bool {
    matches!(
        element,
        TsType::Array { readonly: true, .. }
            | TsType::Fn { .. }
            | TsType::Union { .. }
            | TsType::Intersection(_)
    )
}

/// One [`TsTypeMember`] rendered inline (no leading indent, no trailing
/// separator) — [`render_type`]'s own `TsType::Object` arm's per-member
/// renderer.
pub(super) fn render_type_member(out: &mut String, member: &TsTypeMember) {
    match member {
        TsTypeMember::Prop {
            name,
            ty,
            optional,
            readonly,
        } => {
            if *readonly {
                out.push_str("readonly ");
            }
            out.push_str(name);
            if *optional {
                out.push('?');
            }
            out.push_str(": ");
            render_type(out, ty);
        }
        TsTypeMember::Method {
            name,
            generics,
            params,
            ret,
            doc: _,
        } => {
            out.push_str(name);
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push_str("): ");
            render_type(out, ret);
        }
        TsTypeMember::Index {
            key_name,
            key_ty,
            value_ty,
        } => {
            out.push('[');
            out.push_str(key_name);
            out.push_str(": ");
            render_type(out, key_ty);
            out.push_str("]: ");
            render_type(out, value_ty);
        }
    }
}

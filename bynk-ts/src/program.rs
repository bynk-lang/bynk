//! [`TsProgram`]/[`TsStmt`] — the tree. P7.5 built it wide enough only for
//! the `Verbatim` escape hatch (Q2, `design/tracks/the-typescript-tree.md`
//! §3.2). P7.8 (#1313) adds the real node algebra ([`TsExpr`]/[`TsType`]/
//! [`TsDecl`], plus real [`TsStmt`] variants) — not the full §7.1 reference
//! sketch as literally written (a variant-name list with almost no
//! field-level design), but the subset `bynk-emit/src/emitter/
//! events_fanout.rs` (Arc C's real next file — P7.8's own proposal
//! corrected the track doc's stale schedule, see `design/tracks/
//! the-typescript-tree.md` §6/§9) concretely needs, grounded against that
//! file's own real shape rather than guessed. Building the rest of the
//! sketch's unvalidated variants now would repeat the exact "guessing, not
//! designing" risk `bynk-ts`'s own module doc (`lib.rs`) already named for
//! this layer — Arc C's later slices add more variants file by file, the
//! same precedent [`VerbatimOrigin`] already set.

mod decl;
mod expr;
mod spans;
mod stmt;
mod types;
mod verbatim;

pub use decl::{TsClassCtor, TsClassField, TsClassMethod, TsDecl, TsParam};
pub use expr::{TsArrowBody, TsBinaryOp, TsExpr, TsLit, TsObjectEntry, TsUnaryOp};
pub(crate) use stmt::TsStmtKind;
pub use stmt::{ExportedName, TsBindingName, TsDeclarator, TsStmt, TsSwitchCase};
pub use types::{TsMappedModifier, TsType, TsTypeMember};
pub use verbatim::VerbatimOrigin;

/// A whole generated TypeScript module, as an ordered sequence of top-level
/// statements. `Vec<TsStmt>`, plain — no richer container yet (P7.6's own
/// `Artefacts { docs: BTreeMap<PathBuf, Document> }` is where a *project's*
/// documents get keyed; this is one document's own tree).
#[derive(Debug, Default)]
pub struct TsProgram {
    pub stmts: Vec<TsStmt>,
}

impl TsProgram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, stmt: TsStmt) {
        self.stmts.push(stmt);
    }

    /// This program without its top-level import declarations (named, namespace
    /// and default): the body an import list is derived from (1A.36a, #3737).
    /// [`crate::unbound_names`] over it is the set the module must import.
    pub fn without_imports(&self) -> TsProgram {
        TsProgram {
            stmts: self
                .stmts
                .iter()
                .filter(|s| {
                    !matches!(
                        &s.kind,
                        TsStmtKind::Decl(
                            TsDecl::Import { .. }
                                | TsDecl::ImportNamespace { .. }
                                | TsDecl::ImportDefault { .. }
                        )
                    )
                })
                .cloned()
                .collect(),
        }
    }

    /// Every local name the top-level import declarations bind: each entry of a
    /// named import (`X`, `type X`, `X as Y` bind `X`, `X`, `Y`), a namespace
    /// alias, and a default import's alias and named entries. Imports written
    /// inside opaque text are not reported.
    pub fn imported_names(&self) -> std::collections::BTreeSet<String> {
        fn local(spec: &str) -> String {
            let spec = spec.trim();
            let spec = spec.strip_prefix("type ").unwrap_or(spec).trim();
            spec.rsplit(" as ")
                .next()
                .unwrap_or(spec)
                .trim()
                .to_string()
        }
        let mut out = std::collections::BTreeSet::new();
        for stmt in &self.stmts {
            let TsStmtKind::Decl(decl) = &stmt.kind else {
                continue;
            };
            match decl {
                TsDecl::Import { names, .. } => out.extend(names.iter().map(|n| local(n))),
                TsDecl::ImportNamespace { alias, .. } => {
                    out.insert(alias.clone());
                }
                TsDecl::ImportDefault { alias, named, .. } => {
                    out.insert(alias.clone());
                    out.extend(named.iter().map(|n| local(n)));
                }
                TsDecl::ReExport { .. }
                | TsDecl::ExportNames { .. }
                | TsDecl::ReExportAll { .. }
                | TsDecl::Export(_)
                | TsDecl::Interface { .. }
                | TsDecl::ConstDecl { .. }
                | TsDecl::Class { .. }
                | TsDecl::Function { .. }
                | TsDecl::TypeAlias { .. }
                | TsDecl::ExportDefault(_)
                | TsDecl::DeclareConst { .. } => {}
            }
        }
        out
    }

    /// Drop every import this program never uses (1A.36b-2): an entry of a named import whose
    /// local name no other statement uses, and a namespace import whose alias none does. A named
    /// import left with no entry goes altogether. The order of what stays is kept, and so is
    /// each import's `type` prefix; a default import is left alone.
    ///
    /// A use is what [`crate::unbound_names_in`] reports over the program without its imports,
    /// so text the tree cannot see (a `Verbatim` or `Raw` statement, bar the names
    /// `raw_using` declares) uses nothing. It is how a module whose imports were assembled from
    /// facts that cannot all hold at once (a fixed header, an import the driver adds for a
    /// feature a body may not reach) ends up importing what its tree uses and no more.
    pub fn prune_unused_imports(&mut self) {
        fn local(spec: &str) -> &str {
            let spec = spec.trim();
            let spec = spec.strip_prefix("type ").unwrap_or(spec).trim();
            spec.rsplit(" as ").next().unwrap_or(spec).trim()
        }
        let used: std::collections::BTreeSet<String> =
            crate::unbound_names_in(&self.without_imports().stmts, |_| true)
                .into_iter()
                .collect();
        self.stmts.retain_mut(|stmt| {
            let TsStmtKind::Decl(decl) = &mut stmt.kind else {
                return true;
            };
            match decl {
                TsDecl::Import { names, .. } => {
                    names.retain(|n| used.contains(local(n)));
                    !names.is_empty()
                }
                TsDecl::ImportNamespace { alias, .. } => used.contains(alias.as_str()),
                TsDecl::ImportDefault { .. }
                | TsDecl::ReExport { .. }
                | TsDecl::ExportNames { .. }
                | TsDecl::ReExportAll { .. }
                | TsDecl::Export(_)
                | TsDecl::Interface { .. }
                | TsDecl::ConstDecl { .. }
                | TsDecl::Class { .. }
                | TsDecl::Function { .. }
                | TsDecl::TypeAlias { .. }
                | TsDecl::ExportDefault(_)
                | TsDecl::DeclareConst { .. } => true,
            }
        });
    }

    /// The local names the top-level named imports from a specifier `from_matches`
    /// accepts bind (`X`, `type X`, `X as Y` bind `X`, `X`, `Y`): what a module takes
    /// from one place, such as the runtime (1A.36b-1).
    pub fn names_imported_from(
        &self,
        from_matches: impl Fn(&str) -> bool,
    ) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for stmt in &self.stmts {
            if let TsStmtKind::Decl(TsDecl::Import { names, from, .. }) = &stmt.kind
                && from_matches(from)
            {
                for spec in names {
                    let spec = spec.trim();
                    let spec = spec.strip_prefix("type ").unwrap_or(spec).trim();
                    out.insert(
                        spec.rsplit(" as ")
                            .next()
                            .unwrap_or(spec)
                            .trim()
                            .to_string(),
                    );
                }
            }
        }
        out
    }

    /// The module specifier of every top-level import declaration (named,
    /// namespace or default), in statement order.
    pub fn import_specifiers(&self) -> Vec<&str> {
        self.stmts
            .iter()
            .filter_map(|s| match &s.kind {
                TsStmtKind::Decl(
                    TsDecl::Import { from, .. }
                    | TsDecl::ImportNamespace { from, .. }
                    | TsDecl::ImportDefault { from, .. },
                ) => Some(from.as_str()),
                TsStmtKind::Verbatim { .. }
                | TsStmtKind::Decl(_)
                | TsStmtKind::Const { .. }
                | TsStmtKind::Let { .. }
                | TsStmtKind::ExprStmt(_)
                | TsStmtKind::Return(_)
                | TsStmtKind::Throw(_)
                | TsStmtKind::If { .. }
                | TsStmtKind::ForOf { .. }
                | TsStmtKind::For { .. }
                | TsStmtKind::TryCatch { .. }
                | TsStmtKind::Block(_)
                | TsStmtKind::Continue(_)
                | TsStmtKind::Break(_)
                | TsStmtKind::While { .. }
                | TsStmtKind::Labelled { .. }
                | TsStmtKind::LetList(_)
                | TsStmtKind::Assign { .. }
                | TsStmtKind::Comment(_)
                | TsStmtKind::DocComment(_)
                | TsStmtKind::Blank
                | TsStmtKind::Switch { .. }
                | TsStmtKind::InlineBlock(_)
                | TsStmtKind::Increment(_)
                | TsStmtKind::Raw(..) => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::span::Span;

    /// `without_export` drops one `export` and nothing else: an exported declaration is its
    /// inner one, a plain declaration is unchanged, and it keeps its own spacing flag.
    #[test]
    fn without_export_unwraps_an_exported_declaration_only() {
        let function = |name: &str| TsDecl::Function {
            name: name.to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body: Vec::new(),
            is_async: false,
            inline: false,
        };
        let mut exported = TsStmt::decl(TsDecl::Export(Box::new(function("f"))), None);
        exported.no_blank_before = true;
        let plain = exported.clone().without_export();
        assert_eq!(plain.kind, TsStmtKind::Decl(function("f")));
        assert!(plain.no_blank_before);
        assert_eq!(
            plain.clone().without_export().kind,
            TsStmtKind::Decl(function("f"))
        );
    }

    /// Only an `await`, a unary operator, a number literal (`Num` or `Raw`) and a spread need
    /// a bracket before a member access; every other kind of expression does not.
    #[test]
    fn receiver_brackets_are_needed_for_await_unary_number_and_spread_only() {
        let id = || TsExpr::Ident("a".to_string());
        for needs in [
            TsExpr::Await(Box::new(id())),
            TsExpr::Unary {
                op: TsUnaryOp::Not,
                expr: Box::new(id()),
            },
            TsExpr::Lit(TsLit::Num("1".to_string())),
            TsExpr::bigint(1),
            TsExpr::Lit(TsLit::Raw("0x1".to_string())),
            TsExpr::Spread(Box::new(id())),
        ] {
            assert!(needs.needs_receiver_brackets(), "{needs:?}");
        }
        for fine in [
            id(),
            TsExpr::Lit(TsLit::Str("s".to_string())),
            TsExpr::Lit(TsLit::Null),
            TsExpr::Lit(TsLit::Bool(true)),
            TsExpr::Paren(Box::new(id())),
            TsExpr::Member {
                object: Box::new(id()),
                property: "p".to_string(),
            },
            TsExpr::Call {
                callee: Box::new(id()),
                args: vec![],
            },
            TsExpr::array(vec![]),
            TsExpr::object(vec![]),
        ] {
            assert!(!fine.needs_receiver_brackets(), "{fine:?}");
        }
    }

    /// The tree compares structurally (1A.35b): two separately built, equal trees are equal, a
    /// difference anywhere inside is not, and a node never equals a different node that prints
    /// the same text. This is what lets a recursion check compare witnesses without printing.
    #[test]
    fn the_tree_compares_structurally() {
        let call = |callee: &str, arg: TsExpr| TsExpr::Call {
            callee: Box::new(TsExpr::Ident(callee.to_string())),
            args: vec![arg],
        };
        let num = |n: &str| TsExpr::Lit(TsLit::Num(n.to_string()));
        assert_eq!(call("f", num("1")), call("f", num("1")));
        assert_ne!(call("f", num("1")), call("f", num("2")));
        assert_ne!(call("f", num("1")), call("g", num("1")));
        // An identifier and a verbatim fragment can print alike and are still different nodes.
        assert_ne!(
            TsExpr::Ident("x".to_string()),
            TsExpr::VerbatimExpr("x".to_string(), VerbatimOrigin::Emit)
        );
        // Types, including an array over a union, compare the same way.
        let list = |t: TsType| TsType::readonly_array(t);
        assert_eq!(list(TsType::named("A")), list(TsType::named("A")));
        assert_ne!(list(TsType::named("A")), TsType::array(TsType::named("A")));
        assert_eq!(
            TsType::named_with_args("Result", vec![TsType::named("A"), TsType::named("B")]),
            TsType::named_with_args("Result", vec![TsType::named("A"), TsType::named("B")])
        );
        // A statement inside an arrow body is part of the comparison.
        let arrow = |ret: TsExpr| TsExpr::Arrow {
            params: Vec::new(),
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Block(vec![TsStmt::return_stmt(
                Some(ret),
                None,
            )])),
        };
        assert_eq!(arrow(num("1")), arrow(num("1")));
        assert_ne!(arrow(num("1")), arrow(num("2")));
    }

    #[test]
    fn verbatim_carries_its_own_span() {
        let span = Span::new(3, 8);
        let stmt = TsStmt::verbatim(VerbatimOrigin::RuntimeUse, "x", Some(span));
        assert_eq!(stmt.span, Some(span));
    }

    #[test]
    fn a_program_prints_its_statements_in_push_order() {
        // Kept as a construction-order check (not a print check — printer.rs
        // owns that) since `TsStmtKind` is `pub(crate)` and no longer
        // exposes a uniform `text()` accessor once non-`Verbatim` kinds
        // exist; span order is what's left to check at this layer.
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(VerbatimOrigin::Contracts, "a", None));
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Secrets,
            "b",
            Some(Span::new(0, 1)),
        ));
        assert_eq!(program.stmts[0].span, None);
        assert_eq!(program.stmts[1].span, Some(Span::new(0, 1)));
    }

    /// Review of #1326, finding 1: `TsExpr::template_lit`'s own
    /// `debug_assert_eq!` must actually fire on a malformed tree, not just
    /// exist as documentation — proves the guard guards, the same "prove
    /// it" discipline this crate applies to every other invariant.
    #[test]
    #[should_panic(expected = "parts.len() must be exprs.len() + 1")]
    fn template_lit_rejects_a_parts_exprs_count_mismatch() {
        let _ = TsExpr::template_lit(
            vec!["a".to_string()],
            vec![TsExpr::Ident("too_many".to_string())],
        );
    }

    /// 1A.36b-2: an import the tree never uses is dropped, entry by entry; what stays keeps its
    /// order and its `type` prefix, and a namespace goes with its alias.
    #[test]
    fn unused_imports_are_pruned_entry_by_entry() {
        let import = |type_only: bool, names: &[&str], from: &str| {
            TsStmt::decl(
                TsDecl::Import {
                    type_only,
                    names: names.iter().map(|n| n.to_string()).collect(),
                    from: from.to_string(),
                },
                None,
            )
        };
        let namespace = |alias: &str, from: &str| {
            TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: false,
                    alias: alias.to_string(),
                    from: from.to_string(),
                },
                None,
            )
        };
        let mut program = TsProgram {
            stmts: vec![
                import(false, &["Ok", "type Result", "Err", "x as y"], "./r.js"),
                import(true, &["Unused"], "./u.js"),
                namespace("used_ns", "./a.js"),
                namespace("unused_ns", "./b.js"),
                TsStmt::expr_stmt(
                    TsExpr::Call {
                        callee: Box::new(TsExpr::Ident("Ok".to_string())),
                        args: vec![TsExpr::Ident("y".to_string())],
                    },
                    None,
                ),
                TsStmt::expr_stmt(
                    TsExpr::Member {
                        object: Box::new(TsExpr::Ident("used_ns".to_string())),
                        property: "f".to_string(),
                    },
                    None,
                ),
                TsStmt::decl(
                    TsDecl::ExportNames {
                        names: vec!["type Result".to_string()],
                    },
                    None,
                ),
            ],
        };
        program.prune_unused_imports();
        let printed = crate::print(&program, "", "", "").text;
        assert!(
            printed.contains("import { Ok, type Result, x as y } from \"./r.js\";"),
            "{printed}"
        );
        assert!(printed.contains("import * as used_ns"), "{printed}");
        assert!(
            !printed.contains("Unused") && !printed.contains("unused_ns"),
            "{printed}"
        );
        assert!(!printed.contains("Err"), "{printed}");
    }
}

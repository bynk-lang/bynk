use super::{TsArrowBody, TsDecl, TsExpr, TsObjectEntry, TsStmt, TsStmtKind};

impl TsStmt {
    /// This statement and every statement nested in it, with no source span. A statement lowered
    /// from one source file that is printed into a module whose map belongs to another (a
    /// contract attack's `requires`, from the target's source, in a test module) clears its
    /// spans so that the map records nothing for it instead of a position in the wrong file.
    pub fn clear_spans(&mut self) {
        self.span = None;
        clear_stmt_kind(&mut self.kind);
    }
}

fn clear_stmts(stmts: &mut [TsStmt]) {
    for s in stmts {
        s.clear_spans();
    }
}

fn clear_stmt_kind(kind: &mut TsStmtKind) {
    match kind {
        TsStmtKind::Verbatim { .. }
        | TsStmtKind::Continue(_)
        | TsStmtKind::Break(_)
        | TsStmtKind::Comment(_)
        | TsStmtKind::DocComment(_)
        | TsStmtKind::Blank
        | TsStmtKind::Raw(..) => {}
        TsStmtKind::Decl(decl) => clear_decl(decl),
        TsStmtKind::Const { init, .. } => clear_expr(init),
        TsStmtKind::Let { init, .. } => {
            if let Some(e) = init {
                clear_expr(e);
            }
        }
        TsStmtKind::ExprStmt(e) | TsStmtKind::Throw(e) | TsStmtKind::Increment(e) => clear_expr(e),
        TsStmtKind::Return(e) => {
            if let Some(e) = e {
                clear_expr(e);
            }
        }
        TsStmtKind::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            clear_expr(cond);
            then_branch.clear_spans();
            if let Some(e) = else_branch {
                e.clear_spans();
            }
        }
        TsStmtKind::ForOf { iter, body, .. } => {
            clear_expr(iter);
            body.clear_spans();
        }
        TsStmtKind::While { cond, body } => {
            clear_expr(cond);
            body.clear_spans();
        }
        TsStmtKind::Labelled { body, .. } => body.clear_spans(),
        TsStmtKind::LetList(decls) => {
            for d in decls {
                if let Some(e) = &mut d.init {
                    clear_expr(e);
                }
            }
        }
        TsStmtKind::For {
            init, test, body, ..
        } => {
            clear_expr(init);
            clear_expr(test);
            body.clear_spans();
        }
        TsStmtKind::TryCatch {
            try_block,
            catch_block,
            finally_block,
            ..
        } => {
            try_block.clear_spans();
            if let Some(c) = catch_block {
                c.clear_spans();
            }
            if let Some(f) = finally_block {
                f.clear_spans();
            }
        }
        TsStmtKind::Block(stmts) | TsStmtKind::InlineBlock(stmts) => clear_stmts(stmts),
        TsStmtKind::Assign { target, value } => {
            clear_expr(target);
            clear_expr(value);
        }
        TsStmtKind::Switch {
            discriminant,
            cases,
        } => {
            clear_expr(discriminant);
            for case in cases {
                case.span = None;
                for t in &mut case.labels {
                    clear_expr(t);
                }
                clear_stmts(&mut case.body);
            }
        }
    }
}

fn clear_decl(decl: &mut TsDecl) {
    match decl {
        TsDecl::Import { .. }
        | TsDecl::ImportNamespace { .. }
        | TsDecl::ImportDefault { .. }
        | TsDecl::ReExport { .. }
        | TsDecl::ExportNames { .. }
        | TsDecl::ReExportAll { .. }
        | TsDecl::Interface { .. }
        | TsDecl::TypeAlias { .. }
        | TsDecl::DeclareConst { .. } => {}
        TsDecl::Export(inner) => clear_decl(inner),
        TsDecl::ConstDecl { init, .. } | TsDecl::ExportDefault(init) => clear_expr(init),
        TsDecl::Class {
            constructor,
            methods,
            end_span,
            ..
        } => {
            *end_span = None;
            if let Some(ctor) = constructor {
                clear_stmts(&mut ctor.body);
            }
            for m in methods {
                m.span = None;
                clear_stmts(&mut m.body);
            }
        }
        TsDecl::Function { body, .. } => clear_stmts(body),
    }
}

fn clear_arrow_body(body: &mut TsArrowBody) {
    match body {
        TsArrowBody::Expr(e) => clear_expr(e),
        TsArrowBody::Block(stmts) | TsArrowBody::Multiline(stmts) | TsArrowBody::Flat(stmts) => {
            clear_stmts(stmts);
        }
    }
}

fn clear_expr(expr: &mut TsExpr) {
    match expr {
        TsExpr::Ident(_) | TsExpr::RawUsing(..) | TsExpr::VerbatimExpr(..) | TsExpr::Lit(_) => {}
        TsExpr::Member { object, .. } | TsExpr::OptionalMember { object, .. } => clear_expr(object),
        TsExpr::TypeApp { callee, .. } => clear_expr(callee),
        TsExpr::Index { object, index } | TsExpr::OptionalIndex { object, index } => {
            clear_expr(object);
            clear_expr(index);
        }
        TsExpr::Arrow { body, .. } => clear_arrow_body(body),
        TsExpr::DefaultIife { default, body, .. } => {
            clear_expr(default);
            clear_stmts(body);
        }
        TsExpr::Call { callee, args }
        | TsExpr::New { callee, args }
        | TsExpr::OptionalCall { callee, args } => {
            clear_expr(callee);
            for a in args {
                clear_expr(a);
            }
        }
        TsExpr::Object { entries, .. } => {
            for e in entries {
                match e {
                    TsObjectEntry::Prop { value, .. } => clear_expr(value),
                    TsObjectEntry::Shorthand(_) => {}
                    TsObjectEntry::Method { body, .. } => clear_stmts(body),
                    TsObjectEntry::Spread(e) => clear_expr(e),
                }
            }
        }
        TsExpr::Array { items, .. } | TsExpr::LinesArray(items) | TsExpr::Sequence(items) => {
            for i in items {
                clear_expr(i);
            }
        }
        TsExpr::TemplateLit { exprs, .. } => {
            for e in exprs {
                clear_expr(e);
            }
        }
        TsExpr::Await(e)
        | TsExpr::AsConst(e)
        | TsExpr::Paren(e)
        | TsExpr::Spread(e)
        | TsExpr::NonNull(e)
        | TsExpr::Yield(e) => clear_expr(e),
        TsExpr::As { expr, .. } | TsExpr::Unary { expr, .. } => clear_expr(expr),
        TsExpr::Binary { left, right, .. } => {
            clear_expr(left);
            clear_expr(right);
        }
        TsExpr::Conditional {
            test,
            consequent,
            alternate,
        }
        | TsExpr::MultilineConditional {
            test,
            consequent,
            alternate,
        } => {
            clear_expr(test);
            clear_expr(consequent);
            clear_expr(alternate);
        }
        TsExpr::Assign { target, value }
        | TsExpr::AddAssign { target, value }
        | TsExpr::NullishAssign { target, value } => {
            clear_expr(target);
            clear_expr(value);
        }
        TsExpr::Function { body, .. } => clear_stmts(body),
    }
}

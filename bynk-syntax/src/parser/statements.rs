//! Function-declaration, block, statement, parameter, and lambda parsing.
//! Split out of `parser.rs` (ADR 0060) as a further `impl Parser` block; the
//! scanning core and the other concerns stay in the parent module, reached
//! as ancestor privates via `self`.

use super::*;

impl<'a> Parser<'a> {
    // -- function declarations --

    pub(crate) fn parse_fn_decl(&mut self) -> Result<FnDecl, CompileError> {
        let kw = self.expect(TokenKind::Fn, "to start a function declaration")?;
        let first = self.expect_ident("after `fn`")?;
        // A method declaration uses `TypeName.methodName`; a free function
        // is just an identifier. Disambiguate on the next token.
        let name = if self.eat(TokenKind::Dot).is_some() {
            let method = self.expect_ident("after `.` in a method declaration")?;
            FnName::Method {
                type_name: first,
                method_name: method,
            }
        } else {
            FnName::Free(first)
        };
        // v0.20a: optional `[A, B]` type parameters (free functions only —
        // generic methods are checked semantically; bounds are rejected here
        // with `bynk.generics.no_bounds`).
        let type_params = self.parse_optional_type_params()?;
        self.expect(TokenKind::LParen, "after the function name")?;
        // For methods, the first parameter may be the special `self` keyword.
        let mut params = Vec::new();
        let mut has_self = false;
        if self.peek_kind() == Some(TokenKind::Self_) {
            let self_tok = self.bump().unwrap();
            if !matches!(name, FnName::Method { .. }) {
                return Err(CompileError::new(
                    "bynk.parse.self_outside_method",
                    self_tok.span,
                    "`self` can only appear as the first parameter of a method declaration",
                )
                .with_note(
                    "use `fn TypeName.method(self, ...)` to declare a method, \
                     or remove `self` for a free function",
                ));
            }
            has_self = true;
            // Allow a trailing comma after `self` for further params.
            if self.peek_kind() == Some(TokenKind::Comma) {
                self.bump();
                if self.peek_kind() != Some(TokenKind::RParen) {
                    params.push(self.parse_param()?);
                    while self.eat(TokenKind::Comma).is_some() {
                        params.push(self.parse_param()?);
                    }
                }
            }
        } else if self.peek_kind() != Some(TokenKind::RParen) {
            params.push(self.parse_param()?);
            while self.eat(TokenKind::Comma).is_some() {
                params.push(self.parse_param()?);
            }
        }
        self.expect(TokenKind::RParen, "to close the parameter list")?;
        self.expect(TokenKind::Arrow, "before the return type")?;
        let return_type = self.parse_type_ref("as the return type")?;
        // v0.115: contract clauses ride between the return type and the body —
        // any number of `requires <name>: <pred>` then `ensures <name>: <pred>`.
        // The two-list split is by keyword, not order; the checker enforces the
        // scoping difference (`result` bound only in `ensures`).
        let mut requires = Vec::new();
        let mut ensures = Vec::new();
        loop {
            match self.peek_kind() {
                Some(TokenKind::Requires) => requires.push(self.parse_contract_clause(true)?),
                Some(TokenKind::Ensures) => ensures.push(self.parse_contract_clause(false)?),
                _ => break,
            }
        }
        let body = self.parse_block("to open the function body")?;
        let span = kw.span.merge(body.span);
        Ok(FnDecl {
            type_params,
            name,
            params,
            return_type,
            requires,
            ensures,
            body,
            has_self,
            documentation: None,
            span,
            trivia: Trivia::default(),
            private: None,
        })
    }

    /// Parse a single contract clause (v0.115): `requires <name>: <pred>` or
    /// `ensures <name>: <pred>`. The predicate is an ordinary expression (with
    /// `implies`/`is`) over the parameters — and, for an `ensures`, the
    /// contextual `result` binding; well-formedness (purity, `Bool`, `result`
    /// scope) is the checker's job, mirroring [`parse_invariant`].
    fn parse_contract_clause(&mut self, is_requires: bool) -> Result<Contract, CompileError> {
        let kw = if is_requires {
            self.expect(TokenKind::Requires, "to start a precondition clause")?
        } else {
            self.expect(TokenKind::Ensures, "to start a postcondition clause")?
        };
        let word = if is_requires { "requires" } else { "ensures" };
        let name = self.expect_ident(&format!("as the clause name after `{word}`"))?;
        self.expect(TokenKind::Colon, "after the contract clause name")?;
        let predicate = self.parse_expr()?;
        let span = kw.span.merge(predicate.span);
        Ok(Contract {
            name,
            predicate,
            span,
        })
    }

    /// Parse a brace-delimited block: `{ statement* expr }` (v0.1 §3.1, v0.5).
    pub(crate) fn parse_block(&mut self, ctx: &str) -> Result<Block, CompileError> {
        let open = self.expect(TokenKind::LBrace, ctx)?;
        self.parse_block_rest(open.span)
    }

    /// v0.118: the body of a block after the opening `{` has been consumed. A
    /// `case` body opens the brace itself, peels its leading `provides` clauses,
    /// then calls this to parse the statements and tail.
    pub(crate) fn parse_block_rest(&mut self, open: Span) -> Result<Block, CompileError> {
        // #2010: each `let … else` in the block counts one level against the
        // nesting budget, as the `match` it stands for would. They are counted
        // here and given back however the block ends, error included.
        let mut let_else_depth = 0usize;
        let result = self.parse_block_rest_counted(open, &mut let_else_depth);
        self.depth -= let_else_depth;
        result
    }

    fn parse_block_rest_counted(
        &mut self,
        open: Span,
        let_else_depth: &mut usize,
    ) -> Result<Block, CompileError> {
        let mut statements = Vec::new();
        // #2010: the `let <pattern> … else …` statements in this block, each
        // with the number of ordinary statements that precede it. Each one
        // takes the rest of the block as its first `match` arm, so they are
        // folded in from the back once the tail is known. The fold is
        // iterative; the nesting budget is charged as each step is parsed.
        let mut let_elses: Vec<(usize, LetElseParts)> = Vec::new();
        // Loop: parse statements until we hit something that's not a statement.
        // v0.1: `let`. v0.5: `let ... <-` is also a statement.
        // v0.7: `assert` is a statement form inside test bodies.
        let tail_leading: Vec<Comment>;
        loop {
            let leading = self.take_leading_trivia();
            // v0.81: `name := expr` is a statement led by an identifier, so it is
            // detected by lookahead rather than a leading keyword.
            let is_statement = matches!(
                self.peek_kind(),
                Some(TokenKind::Let)
                    | Some(TokenKind::Expect)
                    | Some(TokenKind::TildeArrow)
                    | Some(TokenKind::Do)
            ) || self.assign_ahead()
                || self.advance_ahead();
            if self.peek_kind() == Some(TokenKind::Let) && self.let_pattern_ahead() {
                self.enter_recursion("this chain of `let … else` steps")?;
                *let_else_depth += 1;
                let mut parts = self.parse_let_else()?;
                parts.leading = leading;
                parts.trailing = self.take_trailing_trivia();
                let_elses.push((statements.len(), parts));
                continue;
            }
            if is_statement {
                let mut stmt = self.parse_statement()?;
                let trailing = self.take_trailing_trivia();
                match &mut stmt {
                    Statement::Let(l) | Statement::EffectLet(l) => {
                        l.trivia.leading = leading;
                        l.trivia.trailing = trailing;
                    }
                    Statement::Expect(a) => {
                        a.trivia.leading = leading;
                        a.trivia.trailing = trailing;
                    }
                    Statement::Send(s) => {
                        s.trivia.leading = leading;
                        s.trivia.trailing = trailing;
                    }
                    Statement::Do(d) => {
                        d.trivia.leading = leading;
                        d.trivia.trailing = trailing;
                    }
                    Statement::Assign(a) => {
                        a.trivia.leading = leading;
                        a.trivia.trailing = trailing;
                    }
                    Statement::Advance(a) => {
                        a.trivia.leading = leading;
                        a.trivia.trailing = trailing;
                    }
                }
                statements.push(stmt);
            } else {
                tail_leading = leading;
                break;
            }
        }
        // v0.7: a block whose last statement is an `assert` may close without an
        // explicit tail expression. v0.146 (ADR 0170): *any* block may — a block
        // that closes right after its statements (or an empty `{}`) carries a
        // synthesised `()` (unit) tail. Against an `Effect[()]` context the
        // tail-position auto-lift wraps it; against a valued context it is an
        // ordinary type mismatch. `implicit_tail` records the synthesis so the
        // formatter omits the `()` (Bynk has no statement terminator, so a
        // printed `()` would re-attach to the last statement on re-parse).
        let (tail, close, implicit_tail) = if self.peek_kind() == Some(TokenKind::RBrace) {
            let close = self.expect(TokenKind::RBrace, "to close the block")?;
            let tail = Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::UnitLit,
                span: close.span,
            };
            (tail, close.span, true)
        } else {
            let tail = self.parse_expr()?;
            let close = self.expect(TokenKind::RBrace, "to close the block")?;
            (tail, close.span, false)
        };
        let mut block = Block {
            statements,
            tail: Box::new(tail),
            span: open.merge(close),
            tail_leading_comments: tail_leading,
            implicit_tail,
        };
        // #2010: fold each `let … else`, innermost (last) first. The statements
        // after it, with the tail, become the block its pattern's arm runs.
        while let Some((at, parts)) = let_elses.pop() {
            let rest_statements = block.statements.split_off(at);
            let rest = Block {
                statements: rest_statements,
                tail: block.tail,
                // From the end of the `let … else` to the block's `}` (not
                // including it): the rest has no braces of its own.
                span: Span {
                    file: parts.span.file,
                    start: parts.span.end,
                    end: close.start,
                },
                tail_leading_comments: std::mem::take(&mut block.tail_leading_comments),
                implicit_tail: block.implicit_tail,
            };
            let (binding, tail) = self.desugar_let_else(parts, rest);
            block.statements.push(binding);
            block.tail = Box::new(tail);
            block.implicit_tail = false;
        }
        Ok(block)
    }

    /// #2010: whether the `let` at the cursor starts a pattern rather than a
    /// binding name. A pattern starts with `Ok`/`Err`/`Some`/`None`, a literal,
    /// `(`, or a name followed by `(` or `.` (`Shipped(at)`, `Status.Pending`).
    /// A bare name stays a binding (`let None = …` is not a binding today
    /// either, since `None` is a keyword), so no existing program changes.
    fn let_pattern_ahead(&self) -> bool {
        let at = |k: usize| self.tokens.get(self.pos + k).map(|t| t.kind);
        match at(1) {
            Some(
                TokenKind::Ok
                | TokenKind::Err
                | TokenKind::Some
                | TokenKind::None
                | TokenKind::IntLit
                | TokenKind::StrLit
                | TokenKind::True
                | TokenKind::False
                | TokenKind::LParen,
            ) => true,
            Some(TokenKind::Minus) => at(2) == Some(TokenKind::IntLit),
            Some(TokenKind::Ident) => matches!(at(2), Some(TokenKind::LParen | TokenKind::Dot)),
            _ => false,
        }
    }

    /// #2010: `let <pattern> (= | <-) <expr> else (<block> | match { <arms> })`.
    fn parse_let_else(&mut self) -> Result<LetElseParts, CompileError> {
        let kw = self.expect(TokenKind::Let, "to start a let statement")?;
        let pattern = self.parse_pattern_top()?;
        if self.peek_kind() == Some(TokenKind::Colon) {
            let colon = self.peek().unwrap().span;
            return Err(CompileError::new(
                "bynk.parse.let_pattern_annotation",
                colon,
                "a `let` with a pattern cannot carry a type annotation",
            )
            .with_note(
                "the pattern's names take their types from the value; to pin the value's type, annotate an ordinary `let` first and match on that",
            ));
        }
        let effectful = match self.peek_kind() {
            Some(TokenKind::LArrow) => {
                self.bump();
                true
            }
            _ => {
                self.expect(TokenKind::Eq, "or `<-` after the `let` pattern")?;
                false
            }
        };
        let value = self.parse_expr()?;
        let principal = if effectful {
            self.parse_call_site_actor()?
        } else {
            None
        };
        if self.peek_kind() != Some(TokenKind::Else) {
            return Err(CompileError::new(
                "bynk.parse.let_pattern_without_else",
                kw.span.merge(value.span),
                "a `let` with a pattern needs an `else` for the values the pattern does not match",
            )
            .with_note(
                "add `else { … }` after the value to say what this block produces when it does not match, or write a `match`",
            ));
        }
        let else_span = self.bump().unwrap().span;
        let (otherwise, end) = if self.peek_kind() == Some(TokenKind::Match) {
            let match_kw = self.bump().unwrap().span;
            self.expect(TokenKind::LBrace, "to open the `else match` arm list")?;
            let mut arms = Vec::new();
            while self.peek_kind() != Some(TokenKind::RBrace) {
                arms.push(self.parse_match_arm()?);
                let _ = self.eat(TokenKind::Comma);
            }
            let close = self.expect(TokenKind::RBrace, "to close the `else match` arm list")?;
            if arms.is_empty() {
                return Err(CompileError::new(
                    "bynk.parse.empty_match",
                    match_kw.merge(close.span),
                    "an `else match` must have at least one arm",
                ));
            }
            (LetElseOtherwise::Arms(arms), close.span)
        } else {
            let block = self.parse_block("to open the `else` block")?;
            let end = block.span;
            (LetElseOtherwise::Block(block), end)
        };
        Ok(LetElseParts {
            kw: kw.span,
            pattern,
            value,
            effectful,
            principal,
            else_span,
            otherwise,
            span: kw.span.merge(end),
            leading: Vec::new(),
            trailing: None,
        })
    }

    /// #2010: the hidden binding and the `match` a `let … else` stands for —
    /// `let __letElse<offset> (<- | =) e` and
    /// `match __letElse<offset> { P => { rest }, _ => E }` (or the `else match`
    /// arms in place of `_ => E`). The binder's spans are empty, at the `let`,
    /// so no editor surface lands on a name the author never wrote.
    fn desugar_let_else(&mut self, parts: LetElseParts, rest: Block) -> (Statement, Expr) {
        let LetElseParts {
            kw,
            pattern,
            value,
            effectful,
            principal,
            else_span,
            otherwise,
            span,
            leading,
            trailing,
        } = parts;
        let at = Span {
            file: kw.file,
            start: kw.start,
            end: kw.start,
        };
        let binder = Ident {
            name: format!("{LET_ELSE_BINDER_PREFIX}{}", kw.start),
            span: at,
        };
        let form = match otherwise {
            LetElseOtherwise::Block(_) => LetElseForm::Block,
            LetElseOtherwise::Arms(_) => LetElseForm::Match,
        };
        let let_stmt = LetStmt {
            name: binder.clone(),
            type_annot: None,
            value,
            principal,
            let_else: Some(LetElse { form, else_span }),
            span,
            trivia: Trivia { leading, trailing },
        };
        let binding = if effectful {
            Statement::EffectLet(let_stmt)
        } else {
            Statement::Let(let_stmt)
        };
        let match_span = kw.merge(rest.span);
        let mut arms = vec![MatchArm {
            span: pattern.span().merge(rest.span),
            pattern,
            guard: None,
            body: MatchBody::Block(rest),
        }];
        match otherwise {
            LetElseOtherwise::Block(block) => arms.push(MatchArm {
                span: else_span.merge(block.span),
                pattern: Pattern::Wildcard(else_span),
                guard: None,
                body: MatchBody::Block(block),
            }),
            LetElseOtherwise::Arms(more) => arms.extend(more),
        }
        let discriminant = Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::Ident(binder),
            span: at,
        };
        let tail = Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::Match {
                discriminant: Box::new(discriminant),
                arms,
            },
            span: match_span,
        };
        (binding, tail)
    }

    /// v0.81: true when the next two tokens are `<ident> :=` — a `Cell` write
    /// statement, which (unlike `let`/`commit`/`~>`) is led by an identifier.
    /// #2137: `advance <Ident> by …` opens an `advance` statement. `advance` is
    /// contextual, and `<Ident> <Ident> by` is no other statement or
    /// expression (`by` is reserved), so the lookahead is unambiguous.
    fn advance_ahead(&self) -> bool {
        let kind = |i: usize| self.tokens.get(self.pos + i).map(|t| t.kind);
        kind(0) == Some(TokenKind::Ident)
            && self
                .tokens
                .get(self.pos)
                .is_some_and(|t| self.slice(t.span) == "advance")
            && kind(1) == Some(TokenKind::Ident)
            && kind(2) == Some(TokenKind::By)
    }

    pub(crate) fn assign_ahead(&self) -> bool {
        self.peek_kind() == Some(TokenKind::Ident)
            && self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(TokenKind::ColonEq)
    }

    /// v0.182 (#664): parse an optional call-site actor clause after an
    /// effect-let value — `by <Actor>` (unit identity) or `by <Actor>(<expr>)`.
    /// Distinct from the handler `by` clause (`parse_by_clause`), which admits a
    /// binder and an actor *sum* but no identity argument. #1812: also read
    /// after a fault claim's subject (`expect <call> by User("bob") faults`).
    pub(crate) fn parse_call_site_actor(&mut self) -> Result<Option<CallSiteActor>, CompileError> {
        if self.peek_kind() != Some(TokenKind::By) {
            return Ok(None);
        }
        let by_kw = self.expect(TokenKind::By, "to start the call-site actor clause")?;
        let actor = self.expect_ident("as the actor after `by` at a call site")?;
        let (identity, claims, end) = if self.peek_kind() == Some(TokenKind::LParen) {
            self.expect(TokenKind::LParen, "after the actor name")?;
            let id = self.parse_expr()?;
            // #2380 (L5): an optional second argument, the token's claims.
            let claims = if self.peek_kind() == Some(TokenKind::Comma) {
                self.expect(TokenKind::Comma, "between the identity and the claims")?;
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };
            let rp = self.expect(TokenKind::RParen, "after the identity value")?;
            (Some(Box::new(id)), claims, rp.span)
        } else {
            (None, None, actor.span)
        };
        let span = by_kw.span.merge(end);
        Ok(Some(CallSiteActor {
            actor,
            identity,
            claims,
            span,
        }))
    }

    pub(crate) fn parse_statement(&mut self) -> Result<Statement, CompileError> {
        if self.assign_ahead() {
            let target = self.expect_ident("as the assignment target")?;
            self.expect(TokenKind::ColonEq, "after the assignment target")?;
            let value = self.parse_expr()?;
            let span = target.span.merge(value.span);
            return Ok(Statement::Assign(AssignStmt {
                target,
                value,
                span,
                trivia: Trivia::default(),
            }));
        }
        if self.advance_ahead() {
            let keyword = self.expect_ident("to start an advance statement")?.span;
            let cap = self.expect_ident("as the capability after `advance`")?;
            self.expect(TokenKind::By, "after the capability in `advance`")?;
            let amount = self.parse_expr()?;
            let span = keyword.merge(amount.span);
            return Ok(Statement::Advance(AdvanceStmt {
                keyword,
                cap,
                amount,
                span,
                trivia: Trivia::default(),
            }));
        }
        if self.peek_kind() == Some(TokenKind::Expect) {
            let kw = self.expect(TokenKind::Expect, "to start an expect statement")?;
            let value = self.parse_expect_body()?;
            let span = kw.span.merge(value.span);
            return Ok(Statement::Expect(ExpectStmt {
                value,
                span,
                trivia: Trivia::default(),
            }));
        }
        if self.peek_kind() == Some(TokenKind::TildeArrow) {
            let kw = self.expect(TokenKind::TildeArrow, "to start an async send statement")?;
            let value = self.parse_expr()?;
            let span = kw.span.merge(value.span);
            return Ok(Statement::Send(SendStmt {
                value,
                span,
                trivia: Trivia::default(),
            }));
        }
        if self.peek_kind() == Some(TokenKind::Do) {
            // v0.146 (ADR 0170): `do e` — perform a unit effect as a statement.
            let kw = self.expect(TokenKind::Do, "to start a `do` statement")?;
            let value = self.parse_expr()?;
            let span = kw.span.merge(value.span);
            return Ok(Statement::Do(DoStmt {
                value,
                span,
                trivia: Trivia::default(),
            }));
        }
        let kw = self.expect(TokenKind::Let, "to start a let statement")?;
        // Allow `_` as a discard name in `let _ = ...` and `let _ <- ...`.
        let name = if self.peek_kind() == Some(TokenKind::Underscore) {
            let t = self.bump().unwrap();
            Ident {
                name: "_".to_string(),
                span: t.span,
            }
        } else {
            self.expect_ident("after `let`")?
        };
        let type_annot = if self.eat(TokenKind::Colon).is_some() {
            Some(self.parse_type_ref("as the let-binding's type annotation")?)
        } else {
            None
        };
        match self.peek_kind() {
            Some(TokenKind::Eq) => {
                self.bump();
                let value = self.parse_expr()?;
                self.reject_else_on_binding(&name)?;
                let span = kw.span.merge(value.span);
                Ok(Statement::Let(LetStmt {
                    name,
                    type_annot,
                    value,
                    principal: None,
                    let_else: None,
                    span,
                    trivia: Trivia::default(),
                }))
            }
            Some(TokenKind::LArrow) => {
                self.bump();
                let value = self.parse_expr()?;
                // v0.182 (#664): an optional call-site `by <Actor>(<identity>)`
                // clause — the test-body form that names the actor a case acts as
                // when it drives a service handler. Distinct from the handler
                // `by` (no argument); parsed only here, in the effect-let arm.
                let principal = self.parse_call_site_actor()?;
                self.reject_else_on_binding(&name)?;
                let end = principal.as_ref().map(|p| p.span).unwrap_or(value.span);
                let span = kw.span.merge(end);
                Ok(Statement::EffectLet(LetStmt {
                    name,
                    type_annot,
                    value,
                    principal,
                    let_else: None,
                    span,
                    trivia: Trivia::default(),
                }))
            }
            Some(_) => {
                let t = self.peek().unwrap();
                Err(CompileError::new(
                    "bynk.parse.expected_token",
                    t.span,
                    format!(
                        "expected `=` or `<-` after the let-binding's name, found {}",
                        t.kind.describe()
                    ),
                ))
            }
            None => Err(CompileError::new(
                "bynk.parse.unexpected_eof",
                self.eof_span(),
                "expected `=` or `<-` after the let-binding's name, found end of file",
            )),
        }
    }

    /// #2010: an `else` after a plain `let name = e` is never reached — a name
    /// matches every value — so it is refused rather than silently ignored.
    fn reject_else_on_binding(&self, name: &Ident) -> Result<(), CompileError> {
        if self.peek_kind() != Some(TokenKind::Else) {
            return Ok(());
        }
        Err(CompileError::new(
            "bynk.parse.let_else_on_binding",
            self.peek().unwrap().span,
            format!(
                "`else` can never run here: `{}` is a plain binding, which matches every value",
                name.name
            ),
        )
        .with_label(name.span, "a name, not a pattern")
        .with_note(
            "to match one variant, write a pattern: `Ok(x)`, `Some(x)`, or a qualified variant such as `Status.Pending`",
        ))
    }

    pub(crate) fn parse_param(&mut self) -> Result<Param, CompileError> {
        let name = self.expect_ident("as a parameter name")?;
        self.expect(TokenKind::Colon, "after the parameter name")?;
        let type_ref = self.parse_type_ref("as the parameter type")?;
        let span = name.span.merge(type_ref.span());
        Ok(Param {
            name,
            type_ref,
            span,
        })
    }

    /// v0.20a: at an `LParen` in primary-expression position, decide whether
    /// a lambda follows: scan to the matching `)` counting paren depth, then
    /// peek one token for `=>`. Terminates at EOF; cost is the distance to
    /// the matching paren (the same class as the record-construction
    /// lookahead).
    pub(crate) fn lambda_ahead(&self) -> bool {
        let mut n = 1;
        let mut depth = 1u32;
        loop {
            match self.tokens.get(self.pos + n).map(|t| t.kind) {
                Some(TokenKind::LParen) => depth += 1,
                Some(TokenKind::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        return self.tokens.get(self.pos + n + 1).map(|t| t.kind)
                            == Some(TokenKind::FatArrow);
                    }
                }
                None => return false,
                _ => {}
            }
            n += 1;
        }
    }

    /// v0.20a: parse `(params) => expr | { block }`. Param annotations are
    /// optional (`(o: Order) => …` / `(o) => …`); the unannotated form relies
    /// on an expected function type at the use site (checked semantically).
    pub(crate) fn parse_lambda(&mut self) -> Result<Expr, CompileError> {
        let open = self.bump().unwrap(); // `(`
        let mut params = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            loop {
                let name = self.expect_ident("as a lambda parameter name")?;
                let mut p_span = name.span;
                let type_ref = if self.eat(TokenKind::Colon).is_some() {
                    let t = self.parse_type_ref("as the lambda parameter type")?;
                    p_span = p_span.merge(t.span());
                    Some(t)
                } else {
                    None
                };
                params.push(LambdaParam {
                    name,
                    type_ref,
                    span: p_span,
                });
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen, "to close the lambda parameter list")?;
        self.expect(TokenKind::FatArrow, "after the lambda parameter list")?;
        let body = if self.peek_kind() == Some(TokenKind::LBrace) {
            let block = self.parse_block("as the lambda body")?;
            let span = block.span;
            Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::Block(block),
                span,
            }
        } else {
            self.parse_expr()?
        };
        let span = open.span.merge(body.span);
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::Lambda(LambdaExpr {
                params,
                body: Box::new(body),
                span,
            }),
            span,
        })
    }
}

/// #2010: a parsed `let <pattern> … else …`, before [`Parser::desugar_let_else`]
/// joins it to the rest of its block.
struct LetElseParts {
    kw: Span,
    pattern: Pattern,
    value: Expr,
    effectful: bool,
    principal: Option<CallSiteActor>,
    else_span: Span,
    otherwise: LetElseOtherwise,
    span: Span,
    leading: Vec<Comment>,
    trailing: Option<String>,
}

enum LetElseOtherwise {
    Block(Block),
    Arms(Vec<MatchArm>),
}

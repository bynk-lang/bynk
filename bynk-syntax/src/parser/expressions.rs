//! Expression and pattern parsing — the precedence ladder (`parse_expr`
//! down through `parse_unary`/`parse_postfix`/`parse_primary`), record
//! construction, `match`/pattern parsing, `if`, and the `Ok`/`Err`
//! expression forms. Split out of `parser.rs` (ADR 0060) as a further
//! `impl Parser` block; the scanning core and the other concerns stay in
//! the parent module, reached as ancestor privates via `self`.

use super::*;
use crate::error::Applicability;

impl<'a> Parser<'a> {
    // -- expressions --

    /// Depth-guarded entry to the expression grammar. Parenthesised
    /// expressions re-enter here from `parse_primary`, and every nested
    /// subexpression (call arguments, record fields, `if`/`match` operands, …)
    /// routes through it, so bounding depth here bounds expression recursion as
    /// a whole (#713). It also resets the no-record-literal restriction (#636);
    /// the unguarded ladder body lives in [`Parser::parse_expr_inner`].
    pub(crate) fn parse_expr(&mut self) -> Result<Expr, CompileError> {
        self.enter_recursion("this expression")?;
        // A fresh `parse_expr` is always a new sub-expression — the operands of
        // a delimited form (parentheses, call arguments, list, record field) or
        // a statement expression. The no-record-literal restriction (#636)
        // applies only to the *spine* of an `if`/`match` condition, so clear it
        // here: a record literal is legal again the moment we descend through a
        // delimiter, e.g. `if f(A { x: 1 }) { … }`. The condition spine reaches
        // the precedence ladder via [`parse_cond_expr`], which bypasses this.
        let prev = self.no_record_literal;
        self.no_record_literal = false;
        let result = self.parse_expr_inner();
        self.no_record_literal = prev;
        self.depth -= 1;
        result
    }

    /// The precedence-ladder entry, shared by [`parse_expr`] (which resets the
    /// no-record-literal restriction first) and [`parse_cond_expr`] (which sets
    /// it). Keeping the ladder here — rather than in `parse_expr` — is what lets
    /// a condition parse its spine under the restriction without also clearing
    /// it on entry.
    fn parse_expr_inner(&mut self) -> Result<Expr, CompileError> {
        // v0.9.1: `assert e` is an expression of type `()`. Parsed at the
        // topmost precedence so `assert x == 1` binds as `assert (x == 1)`.
        // In statement position the block parser still consumes `assert` as
        // a Statement::Expect (preserving the v0.7+ form), so this production
        // only fires when `assert` appears in true expression position
        // (e.g., a match-arm body).
        if self.peek_kind() == Some(TokenKind::Expect) {
            let kw = self.expect(TokenKind::Expect, "to start an expect expression")?;
            let value = self.parse_expect_body()?;
            let span = kw.span.merge(value.span);
            return Ok(Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::Expect(Box::new(value)),
                span,
            });
        }
        self.parse_implies()
    }

    /// Parse an expression in **condition position** — an `if` condition or a
    /// `match` discriminant — where a trailing `{` opens the branch/arm block,
    /// not a record literal (#636). `if ready { result } else { … }` must read
    /// `ready` as the condition and `{ result }` as the then-branch, even
    /// though `ready { result }` is also the shape of a shorthand-field record
    /// construction.
    ///
    /// The restriction is set for the condition's own spine only, then
    /// restored. It reaches the precedence ladder through [`parse_expr_inner`],
    /// bypassing [`parse_expr`]'s reset; any delimited sub-expression within the
    /// condition still goes through `parse_expr` and so lifts the restriction
    /// (`if (ready { x }) { … }` constructs a record). Mirrors Rust's
    /// `NO_STRUCT_LITERAL` restriction; parenthesise to construct a record in
    /// condition head position, e.g. `match (A { x: 1 }) { … }`.
    fn parse_cond_expr(&mut self) -> Result<Expr, CompileError> {
        let prev = self.no_record_literal;
        self.no_record_literal = true;
        let result = self.parse_expr_inner();
        self.no_record_literal = prev;
        result
    }

    /// The subject of an `expect` (v0.117): either an observation over a
    /// consumed capability's recorded calls (`Cap.op called …`, `Cap.op never
    /// called`, `A.op before B.op`), a fault claim over an effectful call
    /// (`<call> faults`, #1706), or an ordinary `Bool` predicate. The
    /// observation shape is detected by a `Cap . op` prefix followed by one of
    /// the contextual words `called` / `never` / `before`; the fault claim by
    /// the contextual word `faults` trailing the parsed subject. All of them
    /// stay ordinary identifiers everywhere else.
    pub(crate) fn parse_expect_body(&mut self) -> Result<Expr, CompileError> {
        if self.peek_kind() == Some(TokenKind::Ident)
            && self.nth_kind(1) == Some(TokenKind::Dot)
            && self.nth_kind(2) == Some(TokenKind::Ident)
            && self.nth_kind(3) == Some(TokenKind::Ident)
            && matches!(self.nth_text(3), "called" | "never" | "before")
        {
            return self.parse_observation();
        }
        let subject = self.parse_expr()?;
        // #1812: a fault claim may carry a call-site principal between its
        // call and `faults`, in the same position as on an effect-let
        // (`let r <- call by User("bob")`). Only a fault claim takes one, so
        // once a `by` is read the `faults` is required.
        let principal = self.parse_call_site_actor()?;
        // #1706: `expect <call> faults`. An expression is never followed by a
        // bare identifier, so a trailing `faults` is unambiguous here — the
        // same contextual-word reading as the observation's `with`/`times`.
        if self.peek_kind() == Some(TokenKind::Ident) && self.nth_text(0) == "faults" {
            self.bump(); // `faults`
            let span = subject.span.merge(self.prev_span());
            return Ok(Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::Faults(Box::new(FaultsExpr {
                    call: subject,
                    principal,
                })),
                span,
            });
        }
        if principal.is_some() {
            let ctx = "after a call-site `by` clause in an `expect` — only a fault claim (`expect <call> by <Actor>(…) faults`) takes one";
            return Err(match self.peek() {
                Some(t) => CompileError::new(
                    "bynk.parse.expected_token",
                    t.span,
                    format!("expected `faults` {ctx}, found {}", t.kind.describe()),
                ),
                None => CompileError::new(
                    "bynk.parse.unexpected_eof",
                    self.eof_span(),
                    format!("expected `faults` {ctx}, found end of file"),
                ),
            });
        }
        Ok(subject)
    }

    /// Parse an observation clause (v0.117): `Cap.op` followed by a matcher —
    /// `called [once | <n> times] [with <pred>]`, `never called`, or
    /// `before Cap.op`. The contextual words are matched by text, not token kind.
    fn parse_observation(&mut self) -> Result<Expr, CompileError> {
        let cap = self.expect_ident("as the observed capability name")?;
        self.expect(
            TokenKind::Dot,
            "after the capability name in an observation",
        )?;
        let op = self.expect_ident("as the observed operation name")?;
        let start = cap.span;
        let word = self.nth_text(0);
        let matcher = match word {
            "never" => {
                let never_tok = self.bump().unwrap(); // `never`
                // `never called`
                if self.peek_kind() == Some(TokenKind::Ident) && self.nth_text(0) == "called" {
                    self.bump();
                } else {
                    return Err(CompileError::new(
                        "bynk.parse.expected_expression",
                        never_tok.span,
                        "expected `called` after `never` in an observation",
                    ));
                }
                ObservationMatcher::NeverCalled
            }
            "called" => {
                self.bump(); // `called`
                // Optional count: `once` or `<int> times`.
                let count = if self.peek_kind() == Some(TokenKind::Ident)
                    && self.nth_text(0) == "once"
                {
                    let once = self.bump().unwrap();
                    Some(Box::new(Expr {
                        id: self.alloc_expr_id(),
                        kind: ExprKind::int_lit(1),
                        span: once.span,
                    }))
                } else if self.peek_kind() == Some(TokenKind::IntLit) {
                    let n = self.parse_primary()?;
                    // `<int> times`
                    if self.peek_kind() == Some(TokenKind::Ident) && self.nth_text(0) == "times" {
                        self.bump();
                    } else {
                        return Err(CompileError::new(
                            "bynk.parse.expected_expression",
                            n.span,
                            "expected `times` after the call count in an observation",
                        ));
                    }
                    Some(Box::new(n))
                } else {
                    None
                };
                // Optional `with <pred>`.
                let with_pred =
                    if self.peek_kind() == Some(TokenKind::Ident) && self.nth_text(0) == "with" {
                        self.bump(); // `with`
                        Some(Box::new(self.parse_expr()?))
                    } else {
                        None
                    };
                ObservationMatcher::Called { count, with_pred }
            }
            "before" => {
                self.bump(); // `before`
                let cap2 = self.expect_ident("as the capability name after `before`")?;
                self.expect(TokenKind::Dot, "after the capability name in `before`")?;
                let op2 = self.expect_ident("as the operation name after `before`")?;
                ObservationMatcher::Before { cap: cap2, op: op2 }
            }
            other => {
                return Err(CompileError::new(
                    "bynk.parse.expected_expression",
                    op.span,
                    format!(
                        "expected an observation matcher (`called`/`never`/`before`), found `{other}`"
                    ),
                ));
            }
        };
        let end = self.prev_span();
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::Observation(Box::new(ObservationExpr { cap, op, matcher })),
            span: start.merge(end),
        })
    }

    /// v0.80: `P implies Q` — logical implication, the lowest-precedence binary
    /// operator (below `||`). Right-associative, so `A implies B implies C`
    /// parses as `A implies (B implies C)` (matching `A → (B → C)`).
    fn parse_implies(&mut self) -> Result<Expr, CompileError> {
        let lhs = self.parse_or()?;
        if self.peek_kind() == Some(TokenKind::Implies) {
            self.bump();
            // `implies` is right-associative, so a chain recurses here rather
            // than looping — guard it as a self-recursive descent so
            // `A implies B implies …` cannot overflow the parser (#714).
            self.enter_recursion("this expression")?;
            let rhs = self.parse_implies();
            self.depth -= 1;
            let rhs = rhs?;
            let span = lhs.span.merge(rhs.span);
            return Ok(Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::BinOp(BinOp::Implies, Box::new(lhs), Box::new(rhs)),
                span,
            });
        }
        Ok(lhs)
    }

    fn parse_or(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_and()?;
        // Associative chains are built iteratively, so each fold is counted
        // against the shared nesting budget rather than a recursive descent
        // (#714); `folds` is unwound from `self.depth` before returning.
        let mut folds = 0usize;
        let result = loop {
            if self.peek_kind() != Some(TokenKind::PipePipe) {
                break Ok(lhs);
            }
            self.bump();
            let rhs = match self.parse_and() {
                Ok(rhs) => rhs,
                Err(e) => break Err(e),
            };
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::BinOp(BinOp::Or, Box::new(lhs), Box::new(rhs)),
                span,
            };
            if let Err(e) = self.enter_chain_fold(&mut folds, lhs.span) {
                break Err(e);
            }
        };
        self.depth -= folds;
        result
    }

    fn parse_and(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_eq()?;
        let mut folds = 0usize;
        let result = loop {
            if self.peek_kind() != Some(TokenKind::AmpAmp) {
                break Ok(lhs);
            }
            self.bump();
            let rhs = match self.parse_eq() {
                Ok(rhs) => rhs,
                Err(e) => break Err(e),
            };
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::BinOp(BinOp::And, Box::new(lhs), Box::new(rhs)),
                span,
            };
            if let Err(e) = self.enter_chain_fold(&mut folds, lhs.span) {
                break Err(e);
            }
        };
        self.depth -= folds;
        result
    }

    fn parse_eq(&mut self) -> Result<Expr, CompileError> {
        let lhs = self.parse_cmp()?;
        // v0.2: the `is` operator sits at the same precedence level as
        // equality but produces a Bool from a pattern test.
        if self.peek_kind() == Some(TokenKind::Is) {
            self.bump();
            // #472: `is`'s pattern is a top-level position — [`Parser::parse_pattern_top`]
            // admits a trailing `where` there the same as a match arm's
            // pattern (a nested payload never does — `r is Ok(_ where P)` is
            // a syntax error, caught below the recursive `parse_pattern`).
            // But a *top-level* refined pattern is rejected here too, at
            // parse time rather than deferred to `check_is`: the tree-sitter
            // grammar cannot admit `refined_pattern` inside `is_expr` at all
            // (D4 in the refined-patterns ADR) — `refinement`'s own
            // `&&`-joined predicate list is ambiguous with the surrounding
            // expression grammar there. A plain top-level `matches!` check
            // suffices even with #474's or-patterns in the mix:
            // `parse_pattern_top` folds `|` *before* checking for `where`
            // (`Refined` always wraps the whole pattern, never appearing as
            // one alternative inside an `Or`), so `Pattern::Refined` can only
            // ever be this expression's outermost pattern shape, not buried
            // inside one. Rejecting it here, not just in the checker, keeps
            // the two parsers in agreement on every input (ADR 0213).
            self.is_pattern_tail = true;
            let pattern = self.parse_pattern_top();
            self.is_pattern_tail = false;
            let pattern = pattern?;
            if matches!(pattern, Pattern::Refined { .. }) {
                return Err(CompileError::new(
                    "bynk.types.is_refined_pattern",
                    pattern.span(),
                    "the `is` operator does not accept a refined pattern here",
                )
                .with_note(
                    "declare a named refined type and use `x is TypeName`, or use a `match` arm",
                ));
            }
            let span = lhs.span.merge(pattern.span());
            return Ok(Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::Is {
                    value: Box::new(lhs),
                    pattern: Box::new(pattern),
                },
                span,
            });
        }
        let op = match self.peek_kind() {
            Some(TokenKind::EqEq) => Some(BinOp::Eq),
            Some(TokenKind::BangEq) => Some(BinOp::NotEq),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let rhs = self.parse_cmp()?;
            // Non-associative: reject a second `==` or `!=` at this level.
            if matches!(
                self.peek_kind(),
                Some(TokenKind::EqEq) | Some(TokenKind::BangEq)
            ) {
                let t = self.peek().unwrap();
                return Err(CompileError::new(
                    "bynk.parse.non_associative",
                    t.span,
                    format!(
                        "`{}` is non-associative; chained equality is not allowed",
                        t.kind.describe().trim_matches('`')
                    ),
                )
                .with_note("parenthesise to disambiguate, e.g. `(a == b) == c`"));
            }
            let span = lhs.span.merge(rhs.span);
            Ok(Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::BinOp(op, Box::new(lhs), Box::new(rhs)),
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_cmp(&mut self) -> Result<Expr, CompileError> {
        let lhs = self.parse_add()?;
        let op = match self.peek_kind() {
            Some(TokenKind::Lt) => Some(BinOp::Lt),
            Some(TokenKind::LtEq) => Some(BinOp::LtEq),
            Some(TokenKind::Gt) => Some(BinOp::Gt),
            Some(TokenKind::GtEq) => Some(BinOp::GtEq),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let rhs = self.parse_add()?;
            if matches!(
                self.peek_kind(),
                Some(TokenKind::Lt)
                    | Some(TokenKind::LtEq)
                    | Some(TokenKind::Gt)
                    | Some(TokenKind::GtEq)
            ) {
                let t = self.peek().unwrap();
                return Err(CompileError::new(
                    "bynk.parse.non_associative",
                    t.span,
                    "comparison operators are non-associative; chained comparison is not allowed",
                )
                .with_note("split the comparison: `a < b && b < c` instead of `a < b < c`"));
            }
            let span = lhs.span.merge(rhs.span);
            Ok(Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::BinOp(op, Box::new(lhs), Box::new(rhs)),
                span,
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_add(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_mul()?;
        let mut folds = 0usize;
        let result = loop {
            let op = match self.peek_kind() {
                Some(TokenKind::Plus) => BinOp::Add,
                Some(TokenKind::Minus) => BinOp::Sub,
                _ => break Ok(lhs),
            };
            // v0.130: a `+`/`-` that begins a new line does not continue the
            // expression — it starts a new construct. Without this, a negative
            // literal pattern in the next match arm (`10` ⏎ `-2 => …`) would be
            // mis-parsed as `10 - 2`. No existing program continues a binary
            // expression with a leading operator on the next line.
            if self.next_token_on_new_line(lhs.span) {
                break Ok(lhs);
            }
            self.bump();
            let rhs = match self.parse_mul() {
                Ok(rhs) => rhs,
                Err(e) => break Err(e),
            };
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::BinOp(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
            if let Err(e) = self.enter_chain_fold(&mut folds, lhs.span) {
                break Err(e);
            }
        };
        self.depth -= folds;
        result
    }

    fn parse_mul(&mut self) -> Result<Expr, CompileError> {
        let mut lhs = self.parse_unary()?;
        let mut folds = 0usize;
        let result = loop {
            let op = match self.peek_kind() {
                Some(TokenKind::Star) => BinOp::Mul,
                Some(TokenKind::Slash) => BinOp::Div,
                _ => break Ok(lhs),
            };
            self.bump();
            let rhs = match self.parse_unary() {
                Ok(rhs) => rhs,
                Err(e) => break Err(e),
            };
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::BinOp(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
            if let Err(e) = self.enter_chain_fold(&mut folds, lhs.span) {
                break Err(e);
            }
        };
        self.depth -= folds;
        result
    }

    fn parse_unary(&mut self) -> Result<Expr, CompileError> {
        // `-`/`!` self-recurse here (`----1`, `!!!x`), and — like parentheses —
        // that recursion bypasses `parse_expr`, so guard it directly on the
        // shared budget or a long run overflows the parser's own stack (#714,
        // sibling of #713). The pass-through to `parse_postfix` is not a nesting
        // level and must not be counted.
        let op = match self.peek_kind() {
            Some(TokenKind::Minus) => UnaryOp::Neg,
            Some(TokenKind::Bang) => UnaryOp::Not,
            _ => return self.parse_postfix(),
        };
        let t = self.bump().unwrap();
        self.enter_recursion("this expression")?;
        let inner = self.parse_unary();
        self.depth -= 1;
        let inner = inner?;
        let span = t.span.merge(inner.span);
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::UnaryOp(op, Box::new(inner)),
            span,
        })
    }

    /// Parse a primary expression and then apply postfix operators (`?`,
    /// `.identifier` field access, `.identifier(args)` method call —
    /// v0.2 §3.7).
    ///
    /// A postfix chain is built iteratively into a left-nested receiver spine
    /// (`a.b.c…`, `f()?.g()…`), so — like the associative operator chains — it
    /// can grow an arbitrarily deep tree without recursing `parse_expr` and hit
    /// the same downstream overflow (#714). The bounded-depth guard
    /// ([`Parser::deepen_spine`]) runs in [`Parser::parse_postfix_inner`]; the
    /// wrapper restores the shared `depth` budget wholesale, which — given the
    /// several error-return paths inside — is cleaner than unwinding per fold.
    fn parse_postfix(&mut self) -> Result<Expr, CompileError> {
        let saved = self.depth;
        let result = self.parse_postfix_inner();
        self.depth = saved;
        result
    }

    fn parse_postfix_inner(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.parse_primary()?;
        loop {
            match self.peek_kind() {
                Some(TokenKind::Question) => {
                    let q = self.bump().unwrap();
                    let span = e.span.merge(q.span);
                    e = Expr {
                        id: self.alloc_expr_id(),
                        kind: ExprKind::Question(Box::new(e)),
                        span,
                    };
                    self.deepen_spine(e.span)?;
                }
                Some(TokenKind::Dot) => {
                    let dot = self.bump().unwrap();
                    // `1.` — a numeric literal followed by `.` and no method
                    // name is a malformed float literal, not a member access
                    // (v0.21 §3). `1.toFloat()` stays a method call.
                    if matches!(
                        e.kind,
                        ExprKind::IntLit { .. }
                            | ExprKind::FloatLit { .. }
                            | ExprKind::DecimalLit { .. }
                    ) && self.peek_kind() != Some(TokenKind::Ident)
                    {
                        return Err(CompileError::new(
                            "bynk.parse.malformed_float_literal",
                            e.span.merge(dot.span),
                            "a float literal needs a digit on both sides of the `.`",
                        )
                        .with_note(format!(
                            "write `{lit}.0` (or call a method: `{lit}.round()`)",
                            lit = &self.source[e.span.range()]
                        )));
                    }
                    let member = self.expect_ident("after `.` in field access or method call")?;
                    // v0.22b: explicit type arguments on a method/static —
                    // `Json.decode[T](…)`. The v0.20b same-line-`[` rule
                    // applies: a `[` opening a new line is a list literal.
                    let type_args = if self.peek_kind() == Some(TokenKind::LBracket)
                        && !self.next_token_on_new_line(member.span)
                    {
                        self.bump();
                        let mut type_args = Vec::new();
                        loop {
                            type_args.push(self.parse_type_ref("as a type argument")?);
                            if self.eat(TokenKind::Comma).is_none() {
                                break;
                            }
                        }
                        let close =
                            self.expect(TokenKind::RBracket, "to close the type-argument list")?;
                        if self.peek_kind() != Some(TokenKind::LParen) {
                            return Err(CompileError::new(
                                "bynk.parse.expected_token",
                                close.span,
                                "type arguments must be followed by an argument list — `name[T](…)`",
                            )
                            .with_note("a bare `name[T]` value form is reserved"));
                        }
                        type_args
                    } else {
                        Vec::new()
                    };
                    // #981: same same-line rule as the bare-identifier call
                    // above — a `(` opening a new line is not this method's
                    // call-parens (`receiver.method` ⏎ `()` stays two
                    // constructs), unless explicit type arguments already
                    // committed to a call being required here.
                    if self.peek_kind() == Some(TokenKind::LParen)
                        && (!type_args.is_empty() || !self.next_token_on_new_line(member.span))
                    {
                        // Method call: `receiver.method(args)`.
                        self.bump();
                        let mut args = Vec::new();
                        if self.peek_kind() != Some(TokenKind::RParen) {
                            args.push(self.parse_expr()?);
                            while self.eat(TokenKind::Comma).is_some() {
                                args.push(self.parse_expr()?);
                            }
                        }
                        let close = self
                            .expect(TokenKind::RParen, "to close the method-call argument list")?;
                        let span = e.span.merge(close.span);
                        e = Expr {
                            id: self.alloc_expr_id(),
                            kind: ExprKind::MethodCall {
                                receiver: Box::new(e),
                                method: member,
                                type_args,
                                args,
                            },
                            span,
                        };
                        self.deepen_spine(e.span)?;
                    } else if let (ExprKind::IntLit { value: v, .. }, Some(unit)) =
                        (&e.kind, DurationUnit::from_name(&member.name))
                    {
                        // v0.86 (ADR 0112): `<int-literal>.<unit>` is a `Duration`
                        // literal, not a field access — an integer-literal
                        // receiver has no fields, so this is unambiguous.
                        let value = *v;
                        let span = e.span.merge(member.span);
                        let millis = value.checked_mul(unit.millis()).ok_or_else(|| {
                            CompileError::new(
                                "bynk.duration.literal_overflow",
                                span,
                                format!(
                                    "the duration `{value}.{}` overflows the millisecond range",
                                    unit.name()
                                ),
                            )
                        })?;
                        e = Expr {
                            id: self.alloc_expr_id(),
                            kind: ExprKind::DurationLit {
                                value,
                                unit,
                                millis,
                            },
                            span,
                        };
                    } else {
                        // Field access: `receiver.field`.
                        let span = e.span.merge(member.span);
                        e = Expr {
                            id: self.alloc_expr_id(),
                            kind: ExprKind::FieldAccess {
                                receiver: Box::new(e),
                                field: member,
                            },
                            span,
                        };
                        self.deepen_spine(e.span)?;
                    }
                }
                _ => break,
            }
        }
        Ok(e)
    }

    /// `Mock '[' type ']' ( '(' args? ')' )?` — v0.9.4 test-context value
    /// construction. The leading `Val` identifier and the `[` lookahead have
    /// already been confirmed by the caller; `kw_span` is the `Val` span.
    fn parse_val_expr(&mut self, kw_span: Span) -> Result<Expr, CompileError> {
        self.expect(TokenKind::LBracket, "after `Val`")?;
        let type_ref = self.parse_type_ref("as the type argument of `Val[T]`")?;
        let close_b = self.expect(TokenKind::RBracket, "to close `Val[T]`")?;
        let mut args = Vec::new();
        let mut end = close_b.span;
        if self.peek_kind() == Some(TokenKind::LParen) {
            self.bump();
            if self.peek_kind() != Some(TokenKind::RParen) {
                args.push(self.parse_expr()?);
                while self.eat(TokenKind::Comma).is_some() {
                    args.push(self.parse_expr()?);
                }
            }
            end = self
                .expect(TokenKind::RParen, "to close the `Val` arguments")?
                .span;
        }
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::Val { type_ref, args },
            span: kw_span.merge(end),
        })
    }

    /// `Wire '(' expr ')'` — Slice C raw system-tier argument. The leading `Wire`
    /// identifier and the `(` lookahead have already been confirmed by the caller;
    /// `kw_span` is the `Wire` span. The inner expression's `String`-ness is a
    /// checker concern, not the parser's.
    fn parse_wire_expr(&mut self, kw_span: Span) -> Result<Expr, CompileError> {
        self.expect(TokenKind::LParen, "after `Wire`")?;
        let inner = self.parse_expr()?;
        let close = self.expect(TokenKind::RParen, "to close `Wire(…)`")?;
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::Wire(Box::new(inner)),
            span: kw_span.merge(close.span),
        })
    }

    fn parse_primary(&mut self) -> Result<Expr, CompileError> {
        let t = self.peek().ok_or_else(|| {
            CompileError::new(
                "bynk.parse.unexpected_eof",
                self.eof_span(),
                "expected an expression, found end of file",
            )
        })?;
        match t.kind {
            TokenKind::IntLit => {
                self.bump();
                let slice = self.slice(t.span);
                // v0.142 (ADR 0166): parse the value from the separator-free form,
                // but keep the as-written lexeme so `fmt` preserves the author's
                // `_` grouping.
                let n: i64 = crate::lexer::strip_digit_separators(slice)
                    .parse()
                    .map_err(|_| {
                        CompileError::new(
                            "bynk.lex.integer_overflow",
                            t.span,
                            format!("integer literal `{slice}` out of 64-bit range"),
                        )
                    })?;
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::IntLit {
                        value: n,
                        lexeme: slice.to_string(),
                    },
                    span: t.span,
                })
            }
            TokenKind::FloatLit => {
                self.bump();
                let slice = self.slice(t.span);
                // tokenize() already validated the separator-free form, so this
                // parse cannot fail — but a silent NaN would corrupt the value
                // if the two ever desync, so fail loudly instead (v0.142).
                let value: f64 = crate::lexer::strip_digit_separators(slice)
                    .parse()
                    .map_err(|_| {
                        CompileError::new(
                            "bynk.lex.float_literal_overflow",
                            t.span,
                            format!("float literal `{slice}` does not parse"),
                        )
                    })?;
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::FloatLit {
                        value,
                        lexeme: slice.to_string(),
                    },
                    span: t.span,
                })
            }
            TokenKind::DecimalLit => {
                self.bump();
                let slice = self.slice(t.span);
                let body = crate::lexer::strip_digit_separators(&slice[..slice.len() - 1]);
                let scale = body.split_once('.').map_or(0, |(_, f)| f.len() as u32);
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::DecimalLit {
                        coef: body.replace('.', ""),
                        scale,
                        lexeme: slice.to_string(),
                    },
                    span: t.span,
                })
            }
            // `.5` — a float literal missing its leading digit (v0.21 §3).
            TokenKind::Dot
                if matches!(
                    self.tokens.get(self.pos + 1).map(|t| t.kind),
                    Some(TokenKind::IntLit | TokenKind::FloatLit)
                ) =>
            {
                let lit = self.tokens[self.pos + 1];
                Err(CompileError::new(
                    "bynk.parse.malformed_float_literal",
                    t.span.merge(lit.span),
                    "a float literal needs a digit on both sides of the `.`",
                )
                .with_note(format!("write `0.{}`", &self.source[lit.span.range()])))
            }
            TokenKind::StrLit => {
                self.bump();
                let s = parse_string_literal(self.slice(t.span), t.span)?;
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::StrLit(s),
                    span: t.span,
                })
            }
            // An interpolated string `"… \(expr) …"` (v0.43). The lexer has
            // already delimited the token and balanced its holes; here we
            // split the chunks from the holes and parse each hole as a full
            // expression (against the original source, so spans stay absolute).
            TokenKind::InterpStr => {
                self.bump();
                let parts = self.parse_interp_parts(t.span)?;
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::InterpStr(parts),
                    span: t.span,
                })
            }
            TokenKind::True => {
                self.bump();
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::BoolLit(true),
                    span: t.span,
                })
            }
            TokenKind::False => {
                self.bump();
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::BoolLit(false),
                    span: t.span,
                })
            }
            TokenKind::LParen => {
                // v0.20a: `(params) => …` — a lambda. Token-level scan to the
                // matching `)` then a one-token peek for `=>`; only paren
                // depth matters (strings are single tokens).
                if self.lambda_ahead() {
                    return self.parse_lambda();
                }
                self.bump();
                // `()` — unit literal (v0.5).
                if self.peek_kind() == Some(TokenKind::RParen) {
                    let close = self.bump().unwrap();
                    return Ok(Expr {
                        id: self.alloc_expr_id(),
                        kind: ExprKind::UnitLit,
                        span: t.span.merge(close.span),
                    });
                }
                let inner = self.parse_expr()?;
                let close =
                    self.expect(TokenKind::RParen, "to close the parenthesised expression")?;
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::Paren(Box::new(inner)),
                    span: t.span.merge(close.span),
                })
            }
            // v0.22a: `Int.parse(…)` / `Float.parse(…)` — a numeric base-type
            // keyword in static-receiver position. Only recognised when the
            // next token is `.`, so a bare `Int` in expression position keeps
            // the ordinary "expected an expression" error. Lowered to an
            // Ident-shaped receiver; postfix builds the MethodCall and the
            // resolver/checker own the static dispatch (like `List.empty()`).
            // v0.86 (ADR 0112): `Duration.millis(…)` is the same static-receiver
            // shape, so the `Duration` keyword joins `Int`/`Float` here. v0.90
            // (ADR 0114): `Instant.fromEpochMillis(…)` joins them.
            TokenKind::Int
            | TokenKind::Float
            | TokenKind::Decimal
            | TokenKind::Duration
            | TokenKind::Instant
            | TokenKind::Bytes
                if self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(TokenKind::Dot) =>
            {
                self.bump();
                let name = self.slice(t.span).to_string();
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::Ident(Ident { name, span: t.span }),
                    span: t.span,
                })
            }
            // `Effect.pure(value)` — wrap a synchronous value as `Effect[T]` (v0.5).
            TokenKind::Effect => {
                let kw = self.bump().unwrap();
                self.expect(TokenKind::Dot, "after `Effect` in `Effect.pure(...)`")?;
                let method = self.expect_ident("after `Effect.`")?;
                if method.name != "pure" {
                    return Err(CompileError::new(
                        "bynk.parse.unknown_effect_method",
                        method.span,
                        format!(
                            "the only operation on `Effect` in expression position is `pure`, but got `{}`",
                            method.name
                        ),
                    )
                    .with_note("use `Effect.pure(value)` to lift a synchronous value into `Effect[T]`"));
                }
                self.expect(TokenKind::LParen, "after `Effect.pure`")?;
                let value = self.parse_expr()?;
                let close =
                    self.expect(TokenKind::RParen, "to close the `Effect.pure` argument")?;
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::EffectPure(Box::new(value)),
                    span: kw.span.merge(close.span),
                })
            }
            // A `RESERVED_CONTEXTUAL` keyword (`asset`/`case`/`event`/
            // `messages`/`on`/`suite`) is reserved only at its own item-dispatch position — an
            // ordinary identifier everywhere else, per `expect_ident`'s
            // exemption (`keywords::RESERVED_CONTEXTUAL`'s doc comment). That
            // exemption covers *declaring* a binding with one of these names
            // (a parameter, a `let`), but not *referencing* it afterwards in
            // expression position, which parses through here instead — a gap
            // latent since `messages`/`on`/`case`/`suite` shipped (no fixture
            // happened to name a binding after one and then read it back) and
            // surfaced concretely by `event` colliding with the pre-existing
            // `examples/event-log` handler `add(event: Event)`.
            TokenKind::Ident
            | TokenKind::Asset
            | TokenKind::Case
            | TokenKind::Const
            | TokenKind::Embed
            | TokenKind::Event
            | TokenKind::Messages
            | TokenKind::On
            | TokenKind::Suite => {
                self.bump();
                let ident = Ident {
                    name: self.slice(t.span).to_string(),
                    span: t.span,
                };
                // v0.9.4: `Val[T]` / `Val[T](args)` — test-context construction.
                if ident.name == "Val" && self.peek_kind() == Some(TokenKind::LBracket) {
                    return self.parse_val_expr(ident.span);
                }
                // Slice C: `Wire(<String>)` — a raw, pre-validation argument to a
                // `system`-tier service address. Recognised by the `Wire (` shape,
                // like `Val[`, so an ordinary call to a user function named `Wire`
                // is still possible only if it never sits in argument position at
                // `system` (the checker restricts `Wire` to that use).
                if ident.name == "Wire" && self.peek_kind() == Some(TokenKind::LParen) {
                    return self.parse_wire_expr(ident.span);
                }
                // v0.117: `trace(Cap.op)` — the observation escape hatch, a
                // test-only builtin. Recognised by the `trace ( Ident . Ident )`
                // shape so an ordinary `trace(value)` call is left untouched.
                if ident.name == "trace"
                    && self.peek_kind() == Some(TokenKind::LParen)
                    && self.nth_kind(1) == Some(TokenKind::Ident)
                    && self.nth_kind(2) == Some(TokenKind::Dot)
                    && self.nth_kind(3) == Some(TokenKind::Ident)
                    && self.nth_kind(4) == Some(TokenKind::RParen)
                {
                    self.bump(); // `(`
                    let cap = self.expect_ident("as the capability name in `trace(Cap.op)`")?;
                    self.expect(TokenKind::Dot, "in `trace(Cap.op)`")?;
                    let op = self.expect_ident("as the operation name in `trace(Cap.op)`")?;
                    let close = self.expect(TokenKind::RParen, "to close `trace(Cap.op)`")?;
                    return Ok(Expr {
                        id: self.alloc_expr_id(),
                        kind: ExprKind::Trace { cap, op },
                        span: ident.span.merge(close.span),
                    });
                }
                // v0.20a: explicit type arguments — `name[T, U](…)`.
                // Bare `name[T]` without an argument list is reserved.
                // v0.20b: the `[` must sit on the same line — a `[` opening
                // a new line starts a list literal, not type application.
                if self.peek_kind() == Some(TokenKind::LBracket)
                    && !self.next_token_on_new_line(ident.span)
                {
                    self.bump();
                    let mut type_args = Vec::new();
                    loop {
                        type_args.push(self.parse_type_ref("as a type argument")?);
                        if self.eat(TokenKind::Comma).is_none() {
                            break;
                        }
                    }
                    let close =
                        self.expect(TokenKind::RBracket, "to close the type-argument list")?;
                    if self.peek_kind() != Some(TokenKind::LParen) {
                        return Err(CompileError::new(
                            "bynk.parse.expected_token",
                            close.span,
                            "type arguments must be followed by an argument list — `name[T](…)`",
                        )
                        .with_note("a bare `name[T]` value form is reserved"));
                    }
                    self.bump();
                    let mut args = Vec::new();
                    if self.peek_kind() != Some(TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        while self.eat(TokenKind::Comma).is_some() {
                            args.push(self.parse_expr()?);
                        }
                    }
                    let close_paren =
                        self.expect(TokenKind::RParen, "to close the argument list")?;
                    return Ok(Expr {
                        id: self.alloc_expr_id(),
                        kind: ExprKind::Call {
                            name: ident.clone(),
                            type_args,
                            args,
                        },
                        span: ident.span.merge(close_paren.span),
                    });
                }
                // #981: a `(` that opens a new line is not this identifier's
                // call-parens — it is a standalone `()` (unit) statement/tail
                // that happens to follow. Without this, `Paid` ⏎ `()` (two
                // statements: a bare identifier and a unit literal) mis-parses
                // as `Paid()` (one statement: a zero-arg call), exactly the
                // same trap the v0.20b same-line-`[` rule dodges for type
                // arguments below.
                if self.peek_kind() == Some(TokenKind::LParen)
                    && !self.next_token_on_new_line(ident.span)
                {
                    self.bump();
                    let mut args = Vec::new();
                    if self.peek_kind() != Some(TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        while self.eat(TokenKind::Comma).is_some() {
                            args.push(self.parse_expr()?);
                        }
                    }
                    let close = self.expect(TokenKind::RParen, "to close the argument list")?;
                    Ok(Expr {
                        id: self.alloc_expr_id(),
                        kind: ExprKind::Call {
                            name: ident.clone(),
                            type_args: Vec::new(),
                            args,
                        },
                        span: ident.span.merge(close.span),
                    })
                } else if self.peek_kind() == Some(TokenKind::LBrace)
                    && !self.no_record_literal
                    && self.looks_like_record_construction()
                {
                    // Record construction: `TypeName { field: value, ... }`.
                    // Suppressed on an `if`/`match` condition spine, where a
                    // trailing `ident {` opens the branch/arm block (#636).
                    self.parse_record_construction(ident)
                } else {
                    Ok(Expr {
                        id: self.alloc_expr_id(),
                        kind: ExprKind::Ident(ident.clone()),
                        span: ident.span,
                    })
                }
            }
            // v0.1: `if cond { ... } else { ... }`.
            TokenKind::If => self.parse_if_expr(),
            // v0.1: `Ok(value)` and `Err(value)` result constructors.
            TokenKind::Ok => self.parse_result_expr(true),
            TokenKind::Err => self.parse_result_expr(false),
            // v0.2: `Some(value)` / `None` / `match` / `self`.
            TokenKind::Some => self.parse_some_expr(),
            TokenKind::None => {
                let tok = self.bump().unwrap();
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::None,
                    span: tok.span,
                })
            }
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::Self_ => {
                // `self` is parsed as a primary identifier with the literal
                // name `self`; the resolver scopes it to method bodies.
                let tok = self.bump().unwrap();
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::Ident(Ident {
                        name: "self".to_string(),
                        span: tok.span,
                    }),
                    span: tok.span,
                })
            }
            // v0.5: bare record spread `{ ...base, field: value }`. Used by
            // `commit { ... }` when the state type is implied.
            TokenKind::LBrace => {
                if self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(TokenKind::DotDotDot) {
                    self.parse_bare_record_spread()
                } else {
                    Err(CompileError::new(
                        "bynk.parse.expected_expression",
                        t.span,
                        "expected an expression, found `{`",
                    )
                    .with_note(
                        "bare record-spread `{ ...base, ... }` is the only `{`-led expression in v0.5; for record construction, use `TypeName { ... }`",
                    ))
                }
            }
            // v0.20b: `[a, b, c]` — list literal. A leading `[` is
            // unambiguous: type application (`name[T](…)`) is parsed as a
            // postfix form on the callee identifier and never reaches here.
            TokenKind::LBracket => {
                let open = self.bump().unwrap();
                let mut elems = Vec::new();
                if self.peek_kind() != Some(TokenKind::RBracket) {
                    loop {
                        elems.push(self.parse_expr()?);
                        if self.eat(TokenKind::Comma).is_none() {
                            break;
                        }
                        // Trailing comma before the closing bracket.
                        if self.peek_kind() == Some(TokenKind::RBracket) {
                            break;
                        }
                    }
                }
                let close = self.expect(TokenKind::RBracket, "to close the list literal")?;
                Ok(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::ListLit(elems),
                    span: open.span.merge(close.span),
                })
            }
            _ => Err(CompileError::new(
                "bynk.parse.expected_expression",
                t.span,
                format!("expected an expression, found {}", t.kind.describe()),
            )),
        }
    }
    /// Lookahead helper: distinguish record construction `T { ... }` from
    /// a `T` ident followed by an unrelated block (which can happen inside
    /// match-arm bodies or if-branches that take a block).
    ///
    /// A record construction has either `Ident :` or `Ident ,` or `Ident }`
    /// after the opening brace, or `}` immediately for the empty case.
    /// A function body or match body never starts with `Ident :` or `Ident ,`
    /// at this position because a `let` would come first as a statement.
    ///
    /// The `Ident }` arm (a single shorthand field, `T { field }`) and the
    /// empty `}` arm are genuinely ambiguous with a block whose whole tail is a
    /// bare identifier and with an empty block — the shapes are identical to a
    /// 2-token lookahead. That ambiguity is resolved by *context*, not here: an
    /// `if`/`match` condition sets `no_record_literal` so the caller never
    /// consults this helper on the condition spine (#636). This predicate is a
    /// pure token-shape test and deliberately stays context-free.
    fn looks_like_record_construction(&self) -> bool {
        debug_assert_eq!(self.peek_kind(), Some(TokenKind::LBrace));
        let a = self.tokens.get(self.pos + 1).map(|t| t.kind);
        let b = self.tokens.get(self.pos + 2).map(|t| t.kind);
        match (a, b) {
            // `T {}` — empty record.
            (Some(TokenKind::RBrace), _) => true,
            // `T { ...base, ... }` — record spread (v0.5).
            (Some(TokenKind::DotDotDot), _) => true,
            // `T { field: ... }` or `T { field, ... }` — record construction.
            (
                Some(TokenKind::Ident),
                Some(TokenKind::Colon) | Some(TokenKind::Comma) | Some(TokenKind::RBrace),
            ) => true,
            _ => false,
        }
    }

    /// Parse `TypeName { field: value, ... }` or `TypeName { ...base, field: value }`
    /// once we've already consumed the type name and the next token is `{`.
    fn parse_record_construction(&mut self, type_name: Ident) -> Result<Expr, CompileError> {
        let open = self.expect(TokenKind::LBrace, "to open the record construction")?;
        // v0.5: spread form `TypeName { ...base, field: value, ... }`.
        if self.peek_kind() == Some(TokenKind::DotDotDot) {
            self.bump();
            let base = self.parse_expr()?;
            let mut overrides = Vec::new();
            while self.eat(TokenKind::Comma).is_some() {
                if self.peek_kind() == Some(TokenKind::RBrace) {
                    break;
                }
                overrides.push(self.parse_field_init()?);
            }
            let close = self.expect(TokenKind::RBrace, "to close the record spread")?;
            let span = type_name.span.merge(close.span);
            return Ok(Expr {
                id: self.alloc_expr_id(),
                kind: ExprKind::RecordSpread {
                    type_name: Some(type_name),
                    base: Box::new(base),
                    overrides,
                },
                span,
            });
        }
        let mut fields = Vec::new();
        while self.peek_kind() != Some(TokenKind::RBrace) {
            fields.push(self.parse_field_init()?);
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        let close = self.expect(TokenKind::RBrace, "to close the record construction")?;
        let span = type_name.span.merge(close.span);
        let _ = open;
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::RecordConstruction { type_name, fields },
            span,
        })
    }

    /// Parse `{ ...base, field: value, ... }` — the bare record-spread form.
    fn parse_bare_record_spread(&mut self) -> Result<Expr, CompileError> {
        let open = self.expect(TokenKind::LBrace, "to open the record spread")?;
        self.expect(TokenKind::DotDotDot, "after `{` in a record spread")?;
        let base = self.parse_expr()?;
        let mut overrides = Vec::new();
        while self.eat(TokenKind::Comma).is_some() {
            if self.peek_kind() == Some(TokenKind::RBrace) {
                break;
            }
            overrides.push(self.parse_field_init()?);
        }
        let close = self.expect(TokenKind::RBrace, "to close the record spread")?;
        let span = open.span.merge(close.span);
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::RecordSpread {
                type_name: None,
                base: Box::new(base),
                overrides,
            },
            span,
        })
    }

    fn parse_field_init(&mut self) -> Result<FieldInit, CompileError> {
        let name = self.expect_ident("as a record-field initialiser name")?;
        // `name : expr` (full form) or `name ,` / `name }` (shorthand).
        if self.eat(TokenKind::Colon).is_some() {
            let value = self.parse_expr()?;
            let span = name.span.merge(value.span);
            Ok(FieldInit {
                name,
                value: Some(value),
                span,
            })
        } else {
            let span = name.span;
            Ok(FieldInit {
                name,
                value: None,
                span,
            })
        }
    }

    /// Parse a `Some(value)` expression.
    fn parse_some_expr(&mut self) -> Result<Expr, CompileError> {
        let kw = self.expect(TokenKind::Some, "to start a `Some` expression")?;
        self.expect(TokenKind::LParen, "after `Some`")?;
        let value = self.parse_expr()?;
        let close = self.expect(TokenKind::RParen, "to close the `Some` argument")?;
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::Some(Box::new(value)),
            span: kw.span.merge(close.span),
        })
    }

    /// Parse a `match` expression: `match expr { pat => body, ... }`.
    fn parse_match_expr(&mut self) -> Result<Expr, CompileError> {
        let kw = self.expect(TokenKind::Match, "to start a match expression")?;
        let discriminant = self.parse_cond_expr()?;
        self.expect(TokenKind::LBrace, "to open the match-arm list")?;
        let mut arms = Vec::new();
        while self.peek_kind() != Some(TokenKind::RBrace) {
            arms.push(self.parse_match_arm()?);
            // No newline check here: the token stream carries no line info, and
            // arms are terminated purely by each arm's own greedy parse plus the
            // final `RBrace`. A trailing comma is optional and otherwise eaten.
            let _ = self.eat(TokenKind::Comma);
        }
        let close = self.expect(TokenKind::RBrace, "to close the match-arm list")?;
        if arms.is_empty() {
            return Err(CompileError::new(
                "bynk.parse.empty_match",
                kw.span.merge(close.span),
                "a `match` expression must have at least one arm",
            ));
        }
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::Match {
                discriminant: Box::new(discriminant),
                arms,
            },
            span: kw.span.merge(close.span),
        })
    }

    pub(crate) fn parse_match_arm(&mut self) -> Result<MatchArm, CompileError> {
        // #472: a match arm's pattern is a top-level position — a trailing
        // `where` is admitted here, but not on a nested payload pattern.
        let pattern = self.parse_pattern_top()?;
        // Optional `if <Bool-expr>` guard between the pattern and `=>` (ADR 0169).
        // The guard sees the arm's bindings; it is unambiguous here because an
        // arm-position `if` can only be a guard.
        let guard = if self.peek_kind() == Some(TokenKind::If) {
            self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::FatArrow, "after a match-arm pattern")?;
        let body = if self.peek_kind() == Some(TokenKind::LBrace) {
            MatchBody::Block(self.parse_block("to open the match-arm body")?)
        } else {
            MatchBody::Expr(self.parse_expr()?)
        };
        let span = pattern.span().merge(body.span());
        Ok(MatchArm {
            pattern,
            guard,
            body,
            span,
        })
    }

    /// Depth-guarded entry to the pattern grammar for a **nested** position —
    /// a variant payload (`parse_pattern` -> `parse_pattern_binding` ->
    /// `parse_pattern`), a third self-recursive descent independent of
    /// `parse_expr`/`parse_type_ref`, so it needs its own guard or a nested
    /// `Ok(Ok(…))` still overflows (#713). Folds `|` alternatives (#474), but
    /// never checks for a trailing `where` (#472) — that suffix is admitted
    /// only at a pattern's top level; see [`Parser::parse_pattern_top`].
    pub(super) fn parse_pattern(&mut self) -> Result<Pattern, CompileError> {
        self.enter_recursion("this pattern")?;
        // Every nested entry is delimited (`(…)`, a payload list, a record
        // pattern's braces), so the `is`-tail restriction does not reach it.
        let tail = std::mem::replace(&mut self.is_pattern_tail, false);
        let result = self.parse_pattern_or();
        self.is_pattern_tail = tail;
        self.depth -= 1;
        result
    }

    /// Depth-guarded entry to the pattern grammar for a **top-level**
    /// position — a match arm's pattern, or `is`'s right-hand side. Folds `|`
    /// alternatives first (#474, [`Parser::parse_pattern_or`]), then checks
    /// the *whole* result for a trailing `where <refinement>` (#472) —
    /// matching the tree-sitter grammar's `refined_pattern: seq(inner:
    /// $._pattern, "where", refinement)`, where `$._pattern` already folds
    /// `|` internally. So `(A | B) where P` is syntactically valid (and
    /// semantically rejected by [`Parser::parse_pattern_where_suffix`]'s
    /// `_`-only rule, the same as any other non-wildcard inner), but a
    /// refined pattern can never be *one alternative* among others in an
    /// outer `|`-chain — `match_arm`'s pattern field is a single
    /// `choice($._pattern, $.refined_pattern)`, not a repetition, so
    /// `(_ where P) | Some(x)` has no tree-sitter shape at all. Checking
    /// `where` per-alternative (inside the fold) rather than once at the end
    /// would admit exactly that unparseable shape — a real grammar/parser
    /// conformance gap caught while resolving this merge (#827/#472 met
    /// #474 for the first time here).
    pub(crate) fn parse_pattern_top(&mut self) -> Result<Pattern, CompileError> {
        self.enter_recursion("this pattern")?;
        let result = self
            .parse_pattern_or()
            .and_then(|base| self.parse_pattern_where_suffix(base));
        self.depth -= 1;
        result
    }

    /// #474: `p₁ | p₂ | … | pₙ` — an or-pattern, left-associative over
    /// [`Parser::parse_pattern_base`] (never a `where`-suffixed alternative —
    /// see [`Parser::parse_pattern_top`]). Built as an iterative chain fold
    /// (like `parse_or`'s fold over boolean `||`, lines 211-237) rather than a
    /// recursive production, so a long chain costs one nesting level total
    /// against the shared budget (#714) instead of one per alternative. `|`
    /// is lexically distinct from `||` and not a valid expression operator,
    /// so there is no ambiguity with the surrounding `is`/match-arm callers.
    fn parse_pattern_or(&mut self) -> Result<Pattern, CompileError> {
        let first = self.parse_pattern_base()?;
        if self.peek_kind() != Some(TokenKind::Pipe) {
            return Ok(first);
        }
        // Parenthesized grouping (`(A | B) | C`) is transparent — matching an
        // or-pattern is associative regardless of how it's grouped — so an
        // already-`Or` atom (from a parenthesized sub-pattern, see the
        // `LParen` case in `parse_pattern_base`) splices its alternatives in
        // rather than nesting, preserving the "an `Or`'s alternatives are
        // always leaves" invariant every `Pattern::Or` consumer relies on.
        let mut alts = flatten_or_alt(first);
        let mut folds = 0usize;
        let result = loop {
            if self.peek_kind() != Some(TokenKind::Pipe) {
                break Ok(());
            }
            self.bump();
            match self.parse_pattern_base() {
                Ok(p) => alts.extend(flatten_or_alt(p)),
                Err(e) => break Err(e),
            }
            if let Err(e) = self.enter_chain_fold(&mut folds, alts.last().unwrap().span()) {
                break Err(e);
            }
        };
        self.depth -= folds;
        result?;
        let span = alts[0].span().merge(alts.last().unwrap().span());
        Ok(Pattern::Or(alts, span))
    }

    /// After a (possibly `|`-chained) top-level pattern, an optional trailing
    /// `where <refinement>` (#472) wraps the *whole* thing — a guard checked
    /// at runtime. v1 admits only a wildcard `_` as the refined inner form;
    /// any other inner shape (including an `Or`) is rejected
    /// (`bynk.parse.refined_pattern_inner`) rather than silently accepted, so
    /// the AST doesn't need reworking once binding/nested forms are designed.
    fn parse_pattern_where_suffix(&mut self, base: Pattern) -> Result<Pattern, CompileError> {
        if self.peek_kind() != Some(TokenKind::Where) {
            return Ok(base);
        }
        self.bump();
        let predicate = self.parse_refinement()?;
        if !matches!(base, Pattern::Wildcard(_)) {
            return Err(CompileError::new(
                "bynk.parse.refined_pattern_inner",
                base.span(),
                "a refined pattern's inner form must be `_` in this version",
            )
            .with_note(
                "write `_ where <predicate>`; binding a refined value is not yet supported",
            ));
        }
        let span = base.span().merge(predicate.span);
        Ok(Pattern::Refined {
            inner: Box::new(base),
            predicate,
            span,
        })
    }

    fn parse_pattern_base(&mut self) -> Result<Pattern, CompileError> {
        if let Some(t) = self.peek() {
            // #474 §2.3.6: parens around a pattern are transparent grouping —
            // `is (Held(...) | Confirmed(...))` — recommended for readability
            // around an or-pattern but not otherwise meaningful (the spec calls
            // them "syntactically optional": `is` already greedily parses one
            // whole pattern, `|`-chain included, with or without them). Recurse
            // through the full `parse_pattern` entry (which folds `|` via
            // `parse_pattern_or`) so a nested `|` chain inside the parens is
            // grouped as one alternative — but never `parse_pattern_top`, so
            // a parenthesized pattern never admits a `where` suffix either
            // (#472; matches tree-sitter's `paren_pattern: seq("(", $._pattern, ")")`,
            // which likewise excludes `refined_pattern`).
            if t.kind == TokenKind::LParen {
                self.bump();
                let inner = self.parse_pattern()?;
                self.expect(TokenKind::RParen, "to close a parenthesized pattern")?;
                return Ok(inner);
            }
            if t.kind == TokenKind::Underscore {
                self.bump();
                return Ok(Pattern::Wildcard(t.span));
            }
            // #2680: a list pattern. `[` begins no other pattern.
            if t.kind == TokenKind::LBracket {
                return self.parse_list_pattern();
            }
            // Literal patterns (v0.130 §2.3.4): `31`, `"english"`, `true`, and a
            // leading unary minus on an integer literal (`-1`). A closed set
            // (ADR 0001) — no `Float`, no `()`.
            // An integer range (#2699): `lo..hi`, `lo..`, `..hi`. Tried first,
            // since a range opens like the literal or the list's `..` it
            // contains.
            if self.int_range_ahead() {
                return Ok(Pattern::Range(self.parse_int_range()?));
            }
            match t.kind {
                TokenKind::IntLit | TokenKind::StrLit | TokenKind::True | TokenKind::False => {
                    return self.parse_literal_pattern();
                }
                TokenKind::Minus
                    if self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(TokenKind::IntLit) =>
                {
                    return self.parse_literal_pattern();
                }
                _ => {}
            }
            // Built-in variant patterns: `Ok(...)`, `Err(...)`, `Some(...)`, `None`.
            match t.kind {
                TokenKind::Ok | TokenKind::Err | TokenKind::Some | TokenKind::None => {
                    return self.parse_variant_pattern_builtin();
                }
                _ => {}
            }
        }
        // Otherwise: an ident-led pattern. Possibly qualified as `Type.Variant`.
        let first = self.expect_ident("as a match-arm pattern")?;
        let (type_name, variant) = if self.eat(TokenKind::Dot).is_some() {
            let v = self.expect_ident("after `.` in a qualified pattern")?;
            (Some(first), v)
        } else {
            (None, first)
        };
        // A bare lowercase identifier with no qualifier and no payload list is a
        // binding (ADR 0169) — `Some(user)`, or a top-level `n if n > 0`. An
        // uppercase-led identifier (or one carrying a payload list) is a variant
        // constructor. Built-in variants are keyword tokens, handled above.
        // #1858: the `(` must sit on the variant's line, as a call's does. A
        // `(` opening a new line starts the next statement: `expect r is None`
        // above a `()` tail is a pattern and a unit, not `None()`.
        let has_payload = self.peek_kind() == Some(TokenKind::LParen)
            && !self.next_token_on_new_line(variant.span);
        // A record pattern (#2641): an uppercase type name directly followed by
        // `{`. Before this, `Name {` could not continue a pattern at all except
        // after `is`, where the `{` opens the `if`'s block.
        if type_name.is_none()
            && !has_payload
            && self.peek_kind() == Some(TokenKind::LBrace)
            && variant
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
        {
            if !self.is_pattern_tail {
                return self.parse_record_pattern(variant);
            }
            if let Some(close) = self.record_pattern_close_ahead() {
                return Err(self.record_pattern_in_is_unparenthesised(variant.span, close));
            }
        }
        if type_name.is_none()
            && !has_payload
            && variant
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase())
        {
            return Ok(Pattern::Binding(variant));
        }
        let mut bindings = Vec::new();
        let mut end_span = variant.span;
        if has_payload {
            self.bump();
            if self.peek_kind() != Some(TokenKind::RParen) {
                bindings.push(self.parse_pattern_binding()?);
                while self.eat(TokenKind::Comma).is_some() {
                    bindings.push(self.parse_pattern_binding()?);
                }
            }
            let close = self.expect(TokenKind::RParen, "to close the pattern binding list")?;
            end_span = close.span;
        }
        let start_span = type_name.as_ref().map(|t| t.span).unwrap_or(variant.span);
        Ok(Pattern::Variant {
            type_name,
            variant,
            bindings,
            span: start_span.merge(end_span),
        })
    }

    /// The index of the `}` closing the brace group at the cursor, when that
    /// group ends in `..` — the shape of a record pattern, which a block never
    /// has (`..` is not an expression). `None` for anything else, including an
    /// unbalanced group. Used only to tell `x is T { f, .. }` (a record pattern
    /// that needs parentheses) from `x is T { body }` (a variant and a block).
    fn record_pattern_close_ahead(&self) -> Option<usize> {
        let mut depth = 0usize;
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            match t.kind {
                TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        let prev = self.tokens.get(i.checked_sub(1)?)?;
                        return (t.kind == TokenKind::RBrace && prev.kind == TokenKind::DotDot)
                            .then_some(i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// `bynk.parse.record_pattern_in_is_unparenthesised`, with the fix that
    /// wraps `Name { … }` (the type name at `name`, the `}` at token `close`)
    /// in parentheses.
    fn record_pattern_in_is_unparenthesised(&self, name: Span, close: usize) -> CompileError {
        let end = self.tokens[close].span;
        let whole = name.merge(end);
        CompileError::new(
            "bynk.parse.record_pattern_in_is_unparenthesised",
            whole,
            "a record pattern after `is` must be parenthesised",
        )
        .with_note(
            "the `{` would otherwise be read as the block that follows the condition; \
             write `x is (Type { field, .. })`",
        )
        .with_suggestion(
            "parenthesise the record pattern",
            vec![
                (
                    Span::new_in(name.file, name.start, name.start),
                    "(".to_string(),
                ),
                (Span::new_in(end.file, end.end, end.end), ")".to_string()),
            ],
            Applicability::MachineApplicable,
        )
    }

    /// Parse a list pattern (#2680): `[]`, `[p, …]`, `[p, …, ..]` or
    /// `[p, …, ..rest]`; the cursor is on the `[`. The `..` is the final
    /// element, at most once, and followed by a name or nothing; `..` followed
    /// by an integer is a range element instead (#2699, patterns track Q7).
    fn parse_list_pattern(&mut self) -> Result<Pattern, CompileError> {
        let open = self.expect(TokenKind::LBracket, "to open a list pattern")?;
        let mut elems = Vec::new();
        let mut rest = None;
        loop {
            match self.peek_kind() {
                Some(TokenKind::RBracket) => break,
                // `..9` is a range element (#2699); `..` then a name or nothing
                // is the rest marker.
                Some(TokenKind::DotDot) if !self.int_range_ahead() => {
                    rest = Some(self.parse_list_rest()?);
                    // A trailing comma after the rest is as harmless as in a list literal.
                    self.eat(TokenKind::Comma);
                    break;
                }
                _ => {}
            }
            elems.push(self.parse_pattern()?);
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        let close = self.expect(TokenKind::RBracket, "to close a list pattern")?;
        Ok(Pattern::List {
            elems,
            rest,
            span: open.span.merge(close.span),
        })
    }

    /// The `..` and optional binder of a list pattern, with the cursor on the
    /// `..`. Leaves the cursor on whatever closes the rest (`]` or a trailing
    /// `,`); anything else is a diagnostic.
    fn parse_list_rest(&mut self) -> Result<ListRest, CompileError> {
        let dots = self.expect(TokenKind::DotDot, "to start a list pattern's rest")?;
        match self.peek_kind() {
            Some(TokenKind::Ident) => {
                let t = self.peek().expect("peeked identifier");
                if self
                    .slice(t.span)
                    .starts_with(|c: char| c.is_ascii_uppercase())
                {
                    return Err(CompileError::new(
                        "bynk.parse.expected_token",
                        t.span,
                        "the name after `..` in a list pattern must start with a lowercase letter",
                    )
                    .with_note(
                        "a capitalised name after `..` is reserved for range patterns, where it would be a named constant bound",
                    ));
                }
                let binder = self.expect_ident("after `..` in a list pattern")?;
                if self.peek_kind() == Some(TokenKind::Comma) {
                    let after = self.tokens.get(self.pos + 1).map(|t| t.kind);
                    if after != Some(TokenKind::RBracket) {
                        return Err(self.list_rest_not_last(dots.span, Some(&binder)));
                    }
                }
                Ok(ListRest {
                    binder: Some(binder),
                    span: dots.span,
                })
            }
            Some(TokenKind::RBracket) => Ok(ListRest {
                binder: None,
                span: dots.span,
            }),
            Some(TokenKind::Comma) => {
                if self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(TokenKind::RBracket) {
                    return Ok(ListRest {
                        binder: None,
                        span: dots.span,
                    });
                }
                Err(self.list_rest_not_last(dots.span, None))
            }
            Some(TokenKind::StrLit | TokenKind::True | TokenKind::False | TokenKind::Minus) => {
                Err(CompileError::new(
                    "bynk.parse.expected_token",
                    dots.span.merge(self.peek().expect("peeked literal").span),
                    "a `..` in a list pattern takes a name, an integer bound, or nothing",
                )
                .with_note(
                    "`..rest` binds the remaining elements and `..9` is an integer range; \
                     only `Int` ranges exist",
                ))
            }
            Some(TokenKind::Underscore) => Err(CompileError::new(
                "bynk.parse.expected_token",
                self.peek().expect("peeked `_`").span,
                "`.._` is not a form: a bare `..` already binds nothing",
            )
            .with_suggestion(
                "drop the `_`",
                vec![(self.peek().expect("peeked `_`").span, String::new())],
                Applicability::MachineApplicable,
            )),
            _ => Err(match self.peek() {
                Some(t) => CompileError::new(
                    "bynk.parse.expected_token",
                    t.span,
                    format!(
                        "expected a name, `,` or `]` after `..` in a list pattern, found {}",
                        t.kind.describe()
                    ),
                ),
                None => CompileError::new(
                    "bynk.parse.expected_token",
                    self.eof_span(),
                    "expected a name, `,` or `]` after `..` in a list pattern, found end of file",
                ),
            }),
        }
    }

    /// `bynk.parse.list_rest_not_last` for a `..` (with its optional binder)
    /// that something follows. The cursor is on the `,` after it. Offers the fix
    /// that moves a bare `..` to the end only when what follows is wildcards.
    fn list_rest_not_last(&mut self, dots: Span, binder: Option<&Ident>) -> CompileError {
        let mut err = CompileError::new(
            "bynk.parse.list_rest_not_last",
            dots,
            "the `..` of a list pattern must be its final element",
        )
        .with_note(
            "patterns that match the end of a list, such as `[.., last]`, are not available",
        );
        if binder.is_some() {
            return err;
        }
        // Wildcards only: `[a, .., _, _]` is `[a, _, _, ..]`.
        self.bump();
        let mut wilds = 0usize;
        let mut last;
        loop {
            match self.peek_kind() {
                Some(TokenKind::Underscore) => {
                    last = self.bump().expect("peeked `_`").span;
                    wilds += 1;
                }
                _ => return err,
            }
            match self.peek_kind() {
                Some(TokenKind::Comma) => {
                    self.bump();
                    if self.peek_kind() == Some(TokenKind::RBracket) {
                        break;
                    }
                }
                Some(TokenKind::RBracket) => break,
                _ => return err,
            }
        }
        let region = dots.merge(last);
        let text = format!("{}, ..", vec!["_"; wilds].join(", "));
        err = err.with_suggestion(
            "move the `..` to the end",
            vec![(region, text)],
            Applicability::MachineApplicable,
        );
        err
    }

    /// Parse the rest of a record pattern (#2641) — `{ f1, f2: pattern, .. }` —
    /// after its type name; the cursor is on the `{`. The trailing `..` is
    /// mandatory: every record pattern is open, so adding a field to the record
    /// never breaks one.
    fn parse_record_pattern(&mut self, type_name: Ident) -> Result<Pattern, CompileError> {
        self.expect(TokenKind::LBrace, "to open a record pattern")?;
        let mut fields: Vec<(Ident, Option<Pattern>)> = Vec::new();
        let mut last_end = None;
        let mut trailing_comma = false;
        let rest_span = loop {
            match self.peek_kind() {
                Some(TokenKind::DotDot) => {
                    break self.bump().expect("peeked `..`").span;
                }
                Some(TokenKind::RBrace) => {
                    let close = self.peek().expect("peeked `}`").span;
                    let (at, text) = match (last_end, trailing_comma) {
                        (Some(end), false) => (end, ", .."),
                        _ => (close.start, ".. "),
                    };
                    return Err(CompileError::new(
                        "bynk.parse.record_pattern_without_rest",
                        close,
                        "a record pattern must end with `..`",
                    )
                    .with_note(
                        "record patterns are always open, so adding a field to the record \
                         never breaks one; the `..` makes that visible",
                    )
                    .with_suggestion(
                        "add the trailing `..`",
                        vec![(Span::new_in(close.file, at, at), text.to_string())],
                        Applicability::MachineApplicable,
                    ));
                }
                _ => {}
            }
            let name = self.expect_ident("as a record pattern field name")?;
            let sub = if self.eat(TokenKind::Colon).is_some() {
                Some(self.parse_pattern()?)
            } else {
                None
            };
            last_end = Some(sub.as_ref().map_or(name.span.end, |p| p.span().end));
            fields.push((name, sub));
            trailing_comma = self.eat(TokenKind::Comma).is_some();
            if !trailing_comma {
                match self.peek_kind() {
                    Some(TokenKind::RBrace) => continue,
                    _ => {
                        return Err(match self.peek() {
                            Some(t) => CompileError::new(
                                "bynk.parse.expected_token",
                                t.span,
                                format!(
                                    "expected `,` or `}}` in a record pattern, found {}",
                                    t.kind.describe()
                                ),
                            ),
                            None => CompileError::new(
                                "bynk.parse.expected_token",
                                self.eof_span(),
                                "expected `,` or `}` in a record pattern, found end of file",
                            ),
                        });
                    }
                }
            }
        };
        let close = self.expect(TokenKind::RBrace, "to close a record pattern")?;
        Ok(Pattern::Record {
            span: type_name.span.merge(close.span),
            type_name,
            fields,
            rest_span,
        })
    }

    /// Whether the cursor is on an integer range (#2699): `lo..`, `lo..hi` or
    /// `..hi`, each bound an optionally negative `IntLit`. The one test every
    /// range position shares (a match pattern, a `let` pattern, a list element,
    /// `via schema(...)`). A `..` followed by anything else is the rest marker.
    pub(crate) fn int_range_ahead(&self) -> bool {
        let at = |k: usize| self.tokens.get(self.pos + k).map(|t| t.kind);
        // How many tokens a bound takes from `k`: `5` or `-5`, else none.
        let bound = |k: usize| match at(k) {
            Some(TokenKind::IntLit) => Some(1),
            Some(TokenKind::Minus) if at(k + 1) == Some(TokenKind::IntLit) => Some(2),
            _ => None,
        };
        match at(0) {
            Some(TokenKind::DotDot) => bound(1).is_some(),
            _ => bound(0).is_some_and(|n| at(n) == Some(TokenKind::DotDot)),
        }
    }

    /// Parse an integer range, with [`Parser::int_range_ahead`] true: the one
    /// production behind a range pattern and `via schema(...)`'s range (#2699,
    /// #990). The high bound is read greedily, so `5.. -1` is the range
    /// `5..-1`. Bounds are parsed permissively: whether they fit an `Int` and
    /// whether `lo <= hi` are the checker's.
    pub(crate) fn parse_int_range(&mut self) -> Result<IntRange, CompileError> {
        let bound_ahead = |p: &Self| {
            matches!(p.peek_kind(), Some(TokenKind::IntLit))
                || (p.peek_kind() == Some(TokenKind::Minus)
                    && p.tokens.get(p.pos + 1).map(|t| t.kind) == Some(TokenKind::IntLit))
        };
        let int_bound = |p: &mut Self| -> Result<(i64, Span), CompileError> {
            match p.parse_literal_value()? {
                (LiteralValue::Int(n), span) => Ok((n, span)),
                _ => unreachable!("an `IntLit` bound parses to an `Int`"),
            }
        };
        let start = self.peek().expect("a range has a first token").span;
        let lo = if bound_ahead(self) {
            Some(int_bound(self)?.0)
        } else {
            None
        };
        let dots = self.expect(TokenKind::DotDot, "in an integer range")?.span;
        let (hi, end) = if bound_ahead(self) {
            let (n, span) = int_bound(self)?;
            (Some(n), span)
        } else {
            (None, dots)
        };
        Ok(IntRange {
            lo,
            hi,
            span: start.merge(end),
        })
    }

    /// Parse a literal pattern (v0.130 §2.3.4): `31`, `-1`, `"english"`,
    /// `true`/`false`. The caller has already confirmed the leading token is one
    /// of these forms.
    fn parse_literal_pattern(&mut self) -> Result<Pattern, CompileError> {
        let (value, span) = self.parse_literal_value()?;
        Ok(Pattern::Literal { value, span })
    }

    /// Parse a bare literal value: `31`, `-1`, `"english"`, `true`/`false`.
    /// The caller has already confirmed the leading token is one of these
    /// forms (see `parse_pattern_base`'s dispatch, and the Events track
    /// slice-1 subscription-pattern value parser, spine #936). Factored out
    /// of `parse_literal_pattern` so both call sites share the integer
    /// overflow diagnostic and digit-separator handling (ADR 0166) rather
    /// than duplicating them.
    pub(crate) fn parse_literal_value(&mut self) -> Result<(LiteralValue, Span), CompileError> {
        // Optional leading `-` on an integer literal (ADR 0001).
        let neg = self.eat(TokenKind::Minus);
        let t = self.bump().expect("literal-value lead token");
        let (value, end_span) = match t.kind {
            TokenKind::IntLit => {
                let slice = self.slice(t.span);
                // v0.142 (ADR 0166): parse from the separator-free form.
                let mut n: i64 = crate::lexer::strip_digit_separators(slice)
                    .parse()
                    .map_err(|_| {
                        CompileError::new(
                            "bynk.lex.integer_overflow",
                            t.span,
                            format!("integer literal `{slice}` out of 64-bit range"),
                        )
                    })?;
                if neg.is_some() {
                    n = -n;
                }
                (LiteralValue::Int(n), t.span)
            }
            TokenKind::StrLit => {
                let s = parse_string_literal(self.slice(t.span), t.span)?;
                (LiteralValue::Str(s), t.span)
            }
            TokenKind::True => (LiteralValue::Bool(true), t.span),
            TokenKind::False => (LiteralValue::Bool(false), t.span),
            _ => unreachable!("parse_literal_value called on a non-literal token"),
        };
        let span = neg.map(|m| m.span).unwrap_or(t.span).merge(end_span);
        Ok((value, span))
    }

    /// Parse a built-in variant pattern (Ok/Err/Some/None) — these are
    /// keyword tokens rather than Idents so they need special handling.
    fn parse_variant_pattern_builtin(&mut self) -> Result<Pattern, CompileError> {
        let t = self.bump().unwrap();
        let variant_name = match t.kind {
            TokenKind::Ok => "Ok",
            TokenKind::Err => "Err",
            TokenKind::Some => "Some",
            TokenKind::None => "None",
            _ => unreachable!(),
        };
        let variant = Ident {
            name: variant_name.to_string(),
            span: t.span,
        };
        let mut bindings = Vec::new();
        let mut end_span = variant.span;
        // #1858: a payload list starts on the variant's line.
        if self.peek_kind() == Some(TokenKind::LParen) && !self.next_token_on_new_line(variant.span)
        {
            self.bump();
            if self.peek_kind() != Some(TokenKind::RParen) {
                bindings.push(self.parse_pattern_binding()?);
                while self.eat(TokenKind::Comma).is_some() {
                    bindings.push(self.parse_pattern_binding()?);
                }
            }
            let close = self.expect(TokenKind::RParen, "to close the pattern binding list")?;
            end_span = close.span;
        }
        let variant_span = variant.span;
        Ok(Pattern::Variant {
            type_name: None,
            variant,
            bindings,
            span: variant_span.merge(end_span),
        })
    }

    fn parse_pattern_binding(&mut self) -> Result<PatternBinding, CompileError> {
        // Allowed shapes (ADR 0169 — each field position is a full sub-pattern):
        //   `_`                 positional wildcard
        //   `name`              positional binding
        //   `Ok(x)` / `Pending` positional nested pattern
        //   `field: <pattern>`  named (where <pattern> is any of the above)
        // A named field is a lowercase-style ident immediately followed by `:`;
        // everything else is a positional sub-pattern.
        if let Some(t) = self.peek()
            && t.kind == TokenKind::Ident
            && self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(TokenKind::Colon)
        {
            let field = self.expect_ident("as a named pattern field")?;
            self.expect(TokenKind::Colon, "after a named pattern field")?;
            let pattern = self.parse_pattern()?;
            let span = field.span.merge(pattern.span());
            return Ok(PatternBinding {
                kind: PatternBindingKind::Named { field, pattern },
                span,
            });
        }
        let pattern = self.parse_pattern()?;
        let span = pattern.span();
        Ok(PatternBinding {
            kind: PatternBindingKind::Positional { pattern },
            span,
        })
    }

    /// Parse `if expr block 'else' (if-expr | block)` (v0.1 §3.2).
    /// Both branches are represented as Blocks; an `else if` chain becomes a
    /// Block whose tail is another If expression.
    fn parse_if_expr(&mut self) -> Result<Expr, CompileError> {
        let kw = self.expect(TokenKind::If, "to start an if expression")?;
        let cond = self.parse_cond_expr()?;
        let then_block = self.parse_block("to open the `if` branch")?;
        // v0.146 (ADR 0170): `else` is optional. A missing `else` defaults to a
        // synthesised `{ () }` (unit) else-branch — legal only when the
        // then-branch is itself unit (`()` / `Effect[()]`), which the checker
        // enforces (`bynk.types.if_without_else_requires_unit`). The synthetic
        // block is marked `implicit_tail` so the formatter round-trips the
        // else-less form. A valued `if` still requires an explicit `else`.
        let else_block = if self.eat(TokenKind::Else).is_some() {
            if self.peek_kind() == Some(TokenKind::If) {
                // `else if ...` desugars to `else { if ... }`.
                let inner = self.parse_if_expr()?;
                let span = inner.span;
                Block {
                    statements: Vec::new(),
                    tail: Box::new(inner),
                    span,
                    tail_leading_comments: Vec::new(),
                    implicit_tail: false,
                }
            } else {
                self.parse_block("to open the `else` branch")?
            }
        } else {
            // Finding #28: a zero-width span at the then-block's end, not
            // `then_block.span` itself — reusing the then-block's span here
            // aliased this synthetic block (and its `UnitLit` tail) with the
            // then-block in `expr_types`, keyed by `Span`, so checking the
            // else branch after the then branch clobbered the then-block's
            // own (more specific) recorded type. Hovering the then-branch of
            // an else-less `if` showed `()` instead of the real `Effect[()]`.
            let synth_span = Span::new(then_block.span.end, then_block.span.end);
            Block {
                statements: Vec::new(),
                tail: Box::new(Expr {
                    id: self.alloc_expr_id(),
                    kind: ExprKind::UnitLit,
                    span: synth_span,
                }),
                span: synth_span,
                tail_leading_comments: Vec::new(),
                implicit_tail: true,
            }
        };
        let span = kw.span.merge(else_block.span);
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_block: Box::new(then_block),
                else_block: Box::new(else_block),
            },
            span,
        })
    }
    /// Parse `Ok(value)` (when `ok` is true) or `Err(error)` (when `ok` is false).
    fn parse_result_expr(&mut self, ok: bool) -> Result<Expr, CompileError> {
        let kw = if ok {
            self.expect(TokenKind::Ok, "to start an `Ok` expression")?
        } else {
            self.expect(TokenKind::Err, "to start an `Err` expression")?
        };
        self.expect(
            TokenKind::LParen,
            if ok { "after `Ok`" } else { "after `Err`" },
        )?;
        let value = self.parse_expr()?;
        let close = self.expect(
            TokenKind::RParen,
            if ok {
                "to close the `Ok` argument"
            } else {
                "to close the `Err` argument"
            },
        )?;
        let span = kw.span.merge(close.span);
        let kind = if ok {
            ExprKind::Ok(Box::new(value))
        } else {
            ExprKind::Err(Box::new(value))
        };
        Ok(Expr {
            id: self.alloc_expr_id(),
            kind,
            span,
        })
    }

    /// Split an `InterpStr` token (covering the whole `"…"`) into its
    /// alternating chunks and holes, parsing each hole as a full expression.
    /// (v0.43.)
    fn parse_interp_parts(&mut self, span: Span) -> Result<Vec<InterpPart>, CompileError> {
        let segments = crate::lexer::split_interp(self.source, span)?;
        let mut parts = Vec::with_capacity(segments.len());
        for segment in segments {
            match segment {
                crate::lexer::InterpSegment::Chunk(text) => parts.push(InterpPart::Chunk(text)),
                crate::lexer::InterpSegment::Hole(hole) => {
                    parts.push(InterpPart::Hole(Box::new(self.parse_hole_expr(hole)?)));
                }
            }
        }
        Ok(parts)
    }

    /// Parse the body of one interpolation hole (`\(expr)`) — the bytes spanned
    /// by `hole` — as a single expression. The hole source is re-lexed and its
    /// token spans are rebased to absolute positions in the full source, so
    /// diagnostics and the (later) LSP point at the real location. (v0.43.)
    fn parse_hole_expr(&mut self, hole: Span) -> Result<Expr, CompileError> {
        let src = &self.source[hole.range()];
        if src.trim().is_empty() {
            return Err(CompileError::new(
                "bynk.parse.empty_interpolation",
                hole,
                "empty interpolation hole",
            )
            .with_note("`\\(…)` must contain an expression"));
        }
        // A lex error here carries spans relative to `src` (the hole substring);
        // rebase them into the full source before propagating, mirroring the
        // success-path token rebasing below. Otherwise the diagnostic points at
        // the file's opening bytes and can split a multibyte char. (#716.)
        let mut tokens =
            crate::lexer::tokenize_in(src, hole.file).map_err(|e| e.offset_spans(hole.start))?;
        for token in &mut tokens {
            token.span = token.span.offset(hole.start);
        }
        let (content, trivia) = split_trivia(&tokens, self.source);
        let mut warnings = Vec::new();
        let mut sub = Parser::new(&content, self.source, trivia, &mut warnings);
        // T3.4 (finding #28, caught live): a fresh `Parser::new` starts its
        // `ExprId` counter at 0, which collides with every id the *outer*
        // parser has already handed out — this hole's sub-parse must
        // continue the same counter, not restart it, or two unrelated nodes
        // end up sharing an id. Carry the counter back out afterward too, so
        // ids the outer parser allocates *after* this hole don't re-collide
        // with ones the sub-parse just handed out.
        sub.next_expr_id = self.next_expr_id;
        let expr = sub.parse_expr()?;
        self.next_expr_id = sub.next_expr_id;
        if let Some(extra) = sub.peek() {
            return Err(CompileError::new(
                "bynk.parse.extra_tokens",
                extra.span,
                format!(
                    "unexpected {} after the interpolation expression",
                    extra.kind.describe()
                ),
            )
            .with_note("an interpolation hole `\\(…)` holds a single expression"));
        }
        Ok(expr)
    }
}

/// #474: the alternatives `p` contributes to an enclosing or-pattern — its own
/// alternatives if it is already `Pattern::Or` (a parenthesized sub-or-pattern
/// being spliced in), otherwise itself alone. Keeps every `Pattern::Or` flat.
fn flatten_or_alt(p: Pattern) -> Vec<Pattern> {
    match p {
        Pattern::Or(alts, _) => alts,
        p => vec![p],
    }
}

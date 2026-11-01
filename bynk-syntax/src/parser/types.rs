//! Type parsing — type declarations (refined/opaque/record/sum), signed
//! numeric literals, and type references (`parse_type_ref`/`parse_type_atom`
//! incl. function-type arrows). Split out of `parser.rs` (ADR 0060) as a
//! further `impl Parser` block; the scanning core and the other concerns
//! stay in the parent module, reached as ancestor privates via `self`.

use super::*;

impl<'a> Parser<'a> {
    // -- type declarations --

    /// v0.20a: parse an optional `[A, B]` type-parameter list. Returns an empty
    /// vector when no `[` follows. A parameter may carry bounds, `[A: Eq]` or
    /// `[A: C1 + C2]` (constrained-generics slice 0); the constraint names are
    /// validated by the checker. Shared by `fn`, `type` and capability-op
    /// declarations so all spell type parameters identically.
    pub(crate) fn parse_optional_type_params(&mut self) -> Result<Vec<TypeParam>, CompileError> {
        let mut type_params = Vec::new();
        if self.peek_kind() == Some(TokenKind::LBracket) {
            self.bump();
            loop {
                let p = self.expect_ident("as a type parameter name")?;
                let mut span = p.span;
                let mut bounds = Vec::new();
                if self.eat(TokenKind::Colon).is_some() {
                    bounds = self.parse_bound_list()?;
                    if let Some(last) = bounds.last() {
                        span = span.merge(last.span);
                    }
                }
                type_params.push(TypeParam {
                    span,
                    name: p,
                    bounds,
                });
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RBracket, "to close the type-parameter list")?;
        }
        Ok(type_params)
    }

    /// `C1 + C2 + …`: the constraint names after a `:` in a bound.
    fn parse_bound_list(&mut self) -> Result<Vec<Ident>, CompileError> {
        let mut bounds = vec![self.expect_ident("as a constraint name after `:`")?];
        while self.eat(TokenKind::Plus).is_some() {
            bounds.push(self.expect_ident("as a constraint name after `+`")?);
        }
        Ok(bounds)
    }

    /// An optional method `where A: C, B: C1 + C2` clause (after the return type).
    pub(crate) fn parse_optional_where_bounds(&mut self) -> Result<Vec<WhereBound>, CompileError> {
        let mut out = Vec::new();
        if self.eat(TokenKind::Where).is_none() {
            return Ok(out);
        }
        loop {
            let param = self.expect_ident("as a type parameter name after `where`")?;
            self.expect(
                TokenKind::Colon,
                "after the type parameter in a `where` clause",
            )?;
            let bounds = self.parse_bound_list()?;
            let span = param
                .span
                .merge(bounds.last().map_or(param.span, |b| b.span));
            out.push(WhereBound {
                param,
                bounds,
                span,
            });
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        Ok(out)
    }

    pub(crate) fn parse_type_decl(&mut self) -> Result<TypeDecl, CompileError> {
        let kw = self.expect(TokenKind::Type, "to start a type declaration")?;
        let name = self.expect_ident("after `type`")?;
        // v0.157 (ADR 0183): a `type` may declare `[T, U]` type parameters,
        // spelled identically to a function's. Only a *record* body may be
        // generic; the checker rejects parameters on refined/opaque/sum bodies.
        let type_params = self.parse_optional_type_params()?;
        self.expect(TokenKind::Eq, "after the type name")?;
        // Dispatch on the head token to decide which kind of type body to parse:
        //   `{ ... }`         → record body (v0.2)
        //   `|` ...           → pipe-form sum (v0.2)
        //   `enum { ... }`    → enum-form sum (v0.2)
        //   `opaque ...`      → opaque base type (v0.3)
        //   anything else     → refined base type (v0)
        let (body, end_span) = match self.peek_kind() {
            Some(TokenKind::LBrace) => {
                let r = self.parse_record_body()?;
                let span = r.span;
                (TypeBody::Record(r), span)
            }
            // #1888: a doc on the first variant comes before its `|`.
            Some(TokenKind::Pipe) => {
                let s = self.parse_sum_body_pipe()?;
                let span = s.span;
                (TypeBody::Sum(s), span)
            }
            Some(TokenKind::DocBlock | TokenKind::DocLine)
                if self.peek_past_docs() == Some(TokenKind::Pipe) =>
            {
                let s = self.parse_sum_body_pipe()?;
                let span = s.span;
                (TypeBody::Sum(s), span)
            }
            Some(TokenKind::Enum) => {
                let s = self.parse_sum_body_enum()?;
                let span = s.span;
                (TypeBody::Sum(s), span)
            }
            Some(TokenKind::Opaque) => {
                self.bump();
                let (base, base_span) = self.parse_base_type()?;
                let mut refinement = None;
                let mut end_span = base_span;
                if self.eat(TokenKind::Where).is_some() {
                    let r = self.parse_refinement()?;
                    end_span = r.span;
                    refinement = Some(r);
                }
                (
                    TypeBody::Opaque {
                        base,
                        base_span,
                        refinement,
                    },
                    end_span,
                )
            }
            _ => {
                let (base, base_span) = self.parse_base_type()?;
                let mut refinement = None;
                let mut end_span = base_span;
                if self.eat(TokenKind::Where).is_some() {
                    let r = self.parse_refinement()?;
                    end_span = r.span;
                    refinement = Some(r);
                }
                (
                    TypeBody::Refined {
                        base,
                        base_span,
                        refinement,
                    },
                    end_span,
                )
            }
        };
        Ok(TypeDecl {
            name,
            type_params,
            body,
            documentation: None,
            span: kw.span.merge(end_span),
            trivia: Trivia::default(),
            private: None,
        })
    }

    /// `event Name @schema(N) = { fields }` (Events track, slice 0, spine
    /// #936; the optional `@schema(N)` annotation is slice 3b, #978). Record
    /// body only — no type parameters, no refined/opaque/sum forms; §7 never
    /// shows any of those for an event, and admitting them is a strictly
    /// later, additive grammar change if ever needed, not a slice-0 gap.
    pub(crate) fn parse_event_decl(&mut self) -> Result<EventDecl, CompileError> {
        let kw = self.expect(TokenKind::Event, "to start an event declaration")?;
        let name = self.expect_ident("after `event`")?;
        // Slice 3b: zero or more `@name(args)` annotations, same permissive
        // shape and placement as `parse_messages_decl`'s own loop (after the
        // declaration's identity, before its body) — the closed registry
        // (today: `@schema` alone) is a checker concern, not a parse error.
        let mut annotations = Vec::new();
        while self.peek_kind() == Some(TokenKind::At) {
            annotations.push(self.parse_annotation()?);
        }
        self.expect(TokenKind::Eq, "after the event name")?;
        let body = self.parse_record_body()?;
        let span = kw.span.merge(body.span);
        Ok(EventDecl {
            name,
            annotations,
            body,
            documentation: None,
            span,
            trivia: Trivia::default(),
        })
    }

    /// Parse the body of a record type: `{ field, field, ... }`.
    /// Each field is `name : type-ref (where refinement)?`; trailing
    /// comma after the last field is allowed.
    ///
    /// #1888: a field may carry a doc above it, in either form.
    fn parse_record_body(&mut self) -> Result<RecordBody, CompileError> {
        let open = self.expect(TokenKind::LBrace, "to open the record body")?;
        let mut fields = Vec::new();
        let mut trailing_comments = Vec::new();
        while self.peek_kind() != Some(TokenKind::RBrace) {
            let (leading, documentation) = self.collect_member_lead(TokenKind::RBrace);
            if self.peek_kind() == Some(TokenKind::RBrace) {
                trailing_comments = leading;
                break;
            }
            let mut field = self.parse_record_field()?;
            let comma = self.eat(TokenKind::Comma);
            field.documentation = documentation;
            field.trivia = Trivia {
                leading,
                trailing: self.take_trailing_trivia(),
            };
            fields.push(field);
            if comma.is_none() {
                break;
            }
        }
        trailing_comments.extend(self.take_leading_trivia());
        let close = self.expect(TokenKind::RBrace, "to close the record body")?;
        Ok(RecordBody {
            fields,
            span: open.span.merge(close.span),
            trailing_comments,
        })
    }

    pub(crate) fn parse_record_field(&mut self) -> Result<RecordField, CompileError> {
        let name = self.expect_ident("as a record field name")?;
        self.expect(TokenKind::Colon, "after the field name")?;
        let type_ref = self.parse_type_ref("as the field type")?;
        let mut refinement = None;
        let mut end_span = type_ref.span();
        if self.eat(TokenKind::Where).is_some() {
            let r = self.parse_refinement()?;
            end_span = r.span;
            refinement = Some(r);
        }
        // v0.11: an optional `= <expr>` initial-value, used by agent state
        // fields. Parsed for every record field; the checker restricts where it
        // is meaningful.
        let mut init = None;
        if self.eat(TokenKind::Eq).is_some() {
            let e = self.parse_expr()?;
            end_span = e.span;
            init = Some(e);
        }
        Ok(RecordField {
            name: name.clone(),
            type_ref,
            refinement,
            init,
            span: name.span.merge(end_span),
            // The caller takes a field's doc, as it does its comments.
            documentation: None,
            // The caller harvests a field's comments: the end-of-line one sits
            // after the `,` this function does not consume.
            trivia: Trivia::default(),
        })
    }

    /// Parse a variant / enum-tag name — an identifier that MUST be
    /// capitalised, matching the grammar's `constant_name`
    /// (`/[A-Z][A-Za-z0-9_]*/`). The `tree-sitter-bynk` grammar cannot lex a
    /// lowercase name in variant position (`constant_name` fails and there is no
    /// `identifier` alternative there), so it rejects `| active` at parse time;
    /// this keeps the compiler parser in agreement rather than accepting a name
    /// the editor grammar refuses (see the cross-parser conformance test).
    fn expect_variant_name(&mut self, ctx: &str) -> Result<Ident, CompileError> {
        let name = self.expect_ident(ctx)?;
        if !name
            .name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
        {
            return Err(CompileError::new(
                "bynk.parse.variant_name_case",
                name.span,
                format!(
                    "variant name `{}` must start with an uppercase letter",
                    name.name
                ),
            )
            .with_note(
                "a sum-type or enum variant is a `constant_name` (`/[A-Z][A-Za-z0-9_]*/`)",
            ));
        }
        Ok(name)
    }

    /// Parse a pipe-form sum body: `| Variant | Variant(field, ...)`.
    /// The leading `|` is required (spec v0.2 §3.2).
    ///
    /// #1794: each variant keeps the comments above it and, unless it is the
    /// last, the one at the end of its line. The last variant's end-of-line
    /// comment is left for `parse_type_decl` to take as the type's own, as it
    /// always was, unless an `embeds` clause follows.
    ///
    /// #1888: a variant may carry a doc above its `|`. A doc after the last
    /// variant is not followed by a `|`, so it is left for the next
    /// declaration.
    fn parse_sum_body_pipe(&mut self) -> Result<SumBody, CompileError> {
        let mut variants = Vec::new();
        let mut span: Option<Span> = None;
        // A comment on the `=` line trails the `=`, which nothing else
        // collects: it leads the first variant, ahead of any above its `|`.
        let mut eq_line: Vec<Comment> = self
            .take_trailing_trivia()
            .map(Comment::Line)
            .into_iter()
            .collect();
        while self.at_pipe_variant() {
            let mut leading = std::mem::take(&mut eq_line);
            let (lead, documentation) = self.collect_member_lead(TokenKind::RBrace);
            leading.extend(lead);
            let bar = self.expect(TokenKind::Pipe, "to start a sum variant")?;
            let name = self.expect_variant_name("after `|` in a sum variant")?;
            let mut payload = Vec::new();
            let mut end_span = name.span;
            if self.peek_kind() == Some(TokenKind::LParen) {
                self.bump();
                // #1888: a payload field may carry a doc above it. The lead is
                // taken here, as `parse_record_body` does, so a doc with only
                // the `)` after it is an orphan warning, not a parse error.
                while self.peek_kind() != Some(TokenKind::RParen) {
                    let (_, documentation) = self.collect_member_lead(TokenKind::RParen);
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        break;
                    }
                    let mut field = self.parse_variant_field()?;
                    field.documentation = documentation;
                    payload.push(field);
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                }
                let close =
                    self.expect(TokenKind::RParen, "to close the variant's payload list")?;
                end_span = close.span;
            }
            let v_span = bar.span.merge(end_span);
            let trailing = if self.at_pipe_variant() || self.at_embeds() {
                self.take_trailing_trivia()
            } else {
                None
            };
            variants.push(Variant {
                name,
                payload,
                span: v_span,
                documentation,
                trivia: Trivia { leading, trailing },
            });
            span = Some(match span {
                Some(s) => s.merge(v_span),
                None => v_span,
            });
        }
        let variants_span = span.expect("parse_sum_body_pipe called without `|`");
        // Comments on their own lines after the last variant lead `embeds`,
        // when it follows; otherwise they lead the next declaration.
        let trailing_comments = if self.at_embeds() {
            self.take_leading_trivia()
        } else {
            Vec::new()
        };
        // v0.154 (ADR 0178): an optional trailing `embeds E as V, …` clause.
        let embeds = self.parse_embeds_clauses()?;
        let span = embeds
            .last()
            .map(|e| variants_span.merge(e.span))
            .unwrap_or(variants_span);
        Ok(SumBody {
            variants,
            embeds,
            span,
            trailing_comments,
        })
    }

    /// #1888: whether a pipe-form variant comes next: a `|`, or a doc above
    /// one.
    fn at_pipe_variant(&self) -> bool {
        match self.peek_kind() {
            Some(TokenKind::Pipe) => true,
            Some(TokenKind::DocBlock | TokenKind::DocLine) => {
                self.peek_past_docs() == Some(TokenKind::Pipe)
            }
            _ => false,
        }
    }

    /// Whether the next token is the contextual `embeds` keyword that opens a
    /// pipe-form sum's trailing clause (v0.154, ADR 0178).
    fn at_embeds(&self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Ident && self.slice(t.span) == "embeds")
    }

    /// Parse the optional trailing `embeds <type> as <Variant>, …` clause of a
    /// pipe-form sum body (v0.154, ADR 0178). `embeds` is a **contextual**
    /// keyword — an identifier literally spelled `embeds` in this trailing
    /// position — so it stays usable as an ordinary identifier elsewhere.
    /// Returns an empty vec when no clause is present.
    fn parse_embeds_clauses(&mut self) -> Result<Vec<EmbedsClause>, CompileError> {
        let mut embeds = Vec::new();
        if !self.at_embeds() {
            return Ok(embeds);
        }
        self.bump(); // consume the `embeds` contextual keyword
        loop {
            let source_type = self.parse_type_ref("as the embedded type in an `embeds` clause")?;
            self.expect(
                TokenKind::As,
                "after the embedded type in an `embeds` clause",
            )?;
            let variant =
                self.expect_variant_name("as the target variant of an `embeds` clause")?;
            let span = source_type.span().merge(variant.span);
            embeds.push(EmbedsClause {
                source_type,
                variant,
                span,
            });
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        Ok(embeds)
    }

    /// Parse an enum-shorthand sum body: `enum { Tag, Tag, Tag }`.
    ///
    /// #1794: as in a record body (#1788), each tag keeps the comments above
    /// it and the one at the end of its line, after its `,`; the comments
    /// before the `}` close the body. A comment on the `=` line leads the first
    /// tag, as in the pipe form, or closes an empty body.
    fn parse_sum_body_enum(&mut self) -> Result<SumBody, CompileError> {
        let mut eq_line: Vec<Comment> = self
            .take_trailing_trivia()
            .map(Comment::Line)
            .into_iter()
            .collect();
        let kw = self.expect(TokenKind::Enum, "to start an enum-form sum body")?;
        self.expect(TokenKind::LBrace, "after `enum`")?;
        let mut variants = Vec::new();
        while self.peek_kind() != Some(TokenKind::RBrace) {
            let mut leading = std::mem::take(&mut eq_line);
            let (lead, documentation) = self.collect_member_lead(TokenKind::RBrace);
            leading.extend(lead);
            if self.peek_kind() == Some(TokenKind::RBrace) {
                eq_line = leading;
                break;
            }
            let name = self.expect_variant_name("as an enum tag name")?;
            let span = name.span;
            let comma = self.eat(TokenKind::Comma);
            variants.push(Variant {
                name,
                payload: Vec::new(),
                span,
                documentation,
                trivia: Trivia {
                    leading,
                    trailing: self.take_trailing_trivia(),
                },
            });
            if comma.is_none() {
                break;
            }
        }
        eq_line.extend(self.take_leading_trivia());
        let trailing_comments = eq_line;
        let close = self.expect(TokenKind::RBrace, "to close the enum body")?;
        Ok(SumBody {
            variants,
            // `enum { … }` variants are payloadless, so they cannot embed a
            // wrapped value — the `embeds` clause is pipe-form only.
            embeds: Vec::new(),
            span: kw.span.merge(close.span),
            trailing_comments,
        })
    }

    /// One payload field, `name: Type`. The caller takes the doc above it
    /// (#1888). A payload field keeps no comments; one above it is dropped
    /// with the lead, and `bynk fmt`'s comment-loss guard refuses rather than
    /// lose it.
    fn parse_variant_field(&mut self) -> Result<VariantField, CompileError> {
        let name = self.expect_ident("as a variant payload field name")?;
        self.expect(TokenKind::Colon, "after the variant payload field name")?;
        let type_ref = self.parse_type_ref("as the variant payload field type")?;
        // #2607 (Decision A): a default belongs to a record's field only. A
        // variant is built like a call, every payload field given, so a default
        // here would be an optional argument by another name.
        if let Some(eq) = self.peek().filter(|t| t.kind == TokenKind::Eq) {
            return Err(CompileError::new(
                "bynk.types.field_default_on_variant",
                eq.span,
                format!(
                    "variant payload field `{}` cannot have a default",
                    name.name
                ),
            )
            .with_note(
                "only a record's fields take defaults; build the variant with every payload \
                 field, or hold the optional values in a record with defaults",
            ));
        }
        let span = name.span.merge(type_ref.span());
        Ok(VariantField {
            name,
            type_ref,
            span,
            // The payload loop attaches the doc above the field.
            documentation: None,
        })
    }

    fn parse_base_type(&mut self) -> Result<(BaseType, Span), CompileError> {
        match self.peek() {
            Some(t) => match t.kind {
                TokenKind::Int => {
                    self.bump();
                    Ok((BaseType::Int, t.span))
                }
                TokenKind::String => {
                    self.bump();
                    Ok((BaseType::String, t.span))
                }
                TokenKind::Bool => {
                    self.bump();
                    Ok((BaseType::Bool, t.span))
                }
                TokenKind::Float => {
                    self.bump();
                    Ok((BaseType::Float, t.span))
                }
                TokenKind::Duration => {
                    self.bump();
                    Ok((BaseType::Duration, t.span))
                }
                TokenKind::Instant => {
                    self.bump();
                    Ok((BaseType::Instant, t.span))
                }
                TokenKind::Bytes => {
                    self.bump();
                    Ok((BaseType::Bytes, t.span))
                }
                _ => Err(CompileError::new(
                    "bynk.parse.expected_base_type",
                    t.span,
                    format!(
                        "expected `Int`, `String`, `Bool`, `Float`, `Duration`, `Instant`, or `Bytes`, found {}",
                        t.kind.describe()
                    ),
                )
                .with_note(
                    "a refined or opaque type is built over one of these base types; a sum starts with `|` (`type S = | A | B`), and a payloadless one can be an `enum { A, B }`",
                )),
            },
            None => Err(CompileError::new(
                "bynk.parse.unexpected_eof",
                self.eof_span(),
                "expected `Int`, `String`, `Bool`, `Float`, `Duration`, `Instant`, or `Bytes`, found end of file",
            )),
        }
    }

    pub(crate) fn parse_refinement(&mut self) -> Result<Refinement, CompileError> {
        // #548: refinement predicates are joined by `&&`, the one conjunction
        // spelling shared with contracts/`expect` (was the `and` keyword before
        // the keyword-hygiene batch). The catalogue stays conjunction-only — no
        // `||`/`!` — so `&&` here is a separator, not a general boolean operator.
        let mut predicates = vec![self.parse_refinement_pred()?];
        let mut span = predicates[0].span;
        while self.eat(TokenKind::AmpAmp).is_some() {
            let p = self.parse_refinement_pred()?;
            span = span.merge(p.span);
            predicates.push(p);
        }
        Ok(Refinement { predicates, span })
    }

    fn parse_refinement_pred(&mut self) -> Result<RefinementPred, CompileError> {
        let t = self.peek().ok_or_else(|| {
            CompileError::new(
                "bynk.parse.unexpected_eof",
                self.eof_span(),
                "expected a refinement predicate, found end of file",
            )
        })?;
        // Allow `Int` etc. through here would be wrong; predicate names are plain
        // identifiers (and not keywords).
        if t.kind != TokenKind::Ident {
            return Err(CompileError::new(
                "bynk.parse.expected_predicate",
                t.span,
                format!(
                    "expected a refinement predicate name, found {}",
                    t.kind.describe()
                ),
            )
            .with_note(
                "valid predicates: Matches, InRange, MinLength, MaxLength, Length, \
                 NonNegative, Positive, NonEmpty",
            ));
        }
        self.bump();
        let name = self.slice(t.span);
        let start = t.span;
        let (kind, end_span) = match name {
            "Matches" => {
                self.expect(TokenKind::LParen, "after `Matches`")?;
                let s_tok = self.expect(TokenKind::StrLit, "as the argument to `Matches`")?;
                let pat = parse_string_literal(self.slice(s_tok.span), s_tok.span)?;
                let close = self.expect(TokenKind::RParen, "after the `Matches` argument")?;
                (PredKind::Matches(pat), close.span)
            }
            "InRange" => {
                self.expect(TokenKind::LParen, "after `InRange`")?;
                let lo = self.parse_signed_num_literal("as the lower bound of `InRange`")?;
                self.expect(TokenKind::Comma, "between `InRange` arguments")?;
                let hi = self.parse_signed_num_literal("as the upper bound of `InRange`")?;
                let close = self.expect(TokenKind::RParen, "after the `InRange` arguments")?;
                let kind = match (lo, hi) {
                    (SignedNumLit::Int(a), SignedNumLit::Int(b)) => PredKind::InRange(a, b),
                    (SignedNumLit::Float(a), SignedNumLit::Float(b)) => PredKind::InRangeF(a, b),
                    _ => {
                        return Err(CompileError::new(
                            "bynk.types.no_numeric_coercion",
                            start.merge(close.span),
                            "`InRange` bounds mix an `Int` literal and a `Float` literal",
                        )
                        .with_note(
                            "both bounds must be the same numeric type — \
                             write `InRange(0, 1)` or `InRange(0.0, 1.0)`",
                        ));
                    }
                };
                (kind, close.span)
            }
            "MinLength" => {
                self.expect(TokenKind::LParen, "after `MinLength`")?;
                let n = self.parse_signed_int_literal("as the argument to `MinLength`")?;
                let close = self.expect(TokenKind::RParen, "after the `MinLength` argument")?;
                (PredKind::MinLength(n), close.span)
            }
            "MaxLength" => {
                self.expect(TokenKind::LParen, "after `MaxLength`")?;
                let n = self.parse_signed_int_literal("as the argument to `MaxLength`")?;
                let close = self.expect(TokenKind::RParen, "after the `MaxLength` argument")?;
                (PredKind::MaxLength(n), close.span)
            }
            "Length" => {
                self.expect(TokenKind::LParen, "after `Length`")?;
                let n = self.parse_signed_int_literal("as the argument to `Length`")?;
                let close = self.expect(TokenKind::RParen, "after the `Length` argument")?;
                (PredKind::Length(n), close.span)
            }
            "NonNegative" => (PredKind::NonNegative, t.span),
            "Positive" => (PredKind::Positive, t.span),
            "NonEmpty" => (PredKind::NonEmpty, t.span),
            other => {
                return Err(CompileError::new(
                    "bynk.parse.unknown_predicate",
                    t.span,
                    format!("unknown refinement predicate `{other}`"),
                )
                .with_note(
                    "valid predicates: Matches, InRange, MinLength, MaxLength, Length, \
                     NonNegative, Positive, NonEmpty",
                ));
            }
        };
        Ok(RefinementPred {
            kind,
            span: start.merge(end_span),
        })
    }

    fn parse_signed_int_literal(&mut self, ctx: &str) -> Result<i64, CompileError> {
        let neg = self.eat(TokenKind::Minus).is_some();
        let t = self.expect(TokenKind::IntLit, ctx)?;
        let slice = self.slice(t.span);
        // v0.142 (ADR 0166): parse from the separator-free form.
        let n: i64 = crate::lexer::strip_digit_separators(slice)
            .parse()
            .map_err(|_| {
                CompileError::new(
                    "bynk.lex.integer_overflow",
                    t.span,
                    format!(
                        "integer literal `{slice}` is out of range for a 64-bit signed integer"
                    ),
                )
            })?;
        Ok(if neg { -n } else { n })
    }

    /// A signed numeric literal in refinement-bound position (v0.21):
    /// either an `Int` or a `Float` literal, optionally negated. The float
    /// form keeps its (signed) lexeme for byte-stable emission.
    fn parse_signed_num_literal(&mut self, ctx: &str) -> Result<SignedNumLit, CompileError> {
        // v0.40 (ADR 0073): the bound span covers a leading `-` so the swap
        // quick-fix replaces `-10` as a unit.
        let minus = self.eat(TokenKind::Minus);
        let neg = minus.is_some();
        let span_of = |lit: Span| match &minus {
            Some(m) => m.span.merge(lit),
            None => lit,
        };
        match self.peek() {
            Some(t) if t.kind == TokenKind::IntLit => {
                self.bump();
                let slice = self.slice(t.span);
                // v0.142 (ADR 0166): parse from the separator-free form.
                let n: i64 = crate::lexer::strip_digit_separators(slice)
                    .parse()
                    .map_err(|_| {
                        CompileError::new(
                            "bynk.lex.integer_overflow",
                            t.span,
                            format!(
                                "integer literal `{slice}` is out of range for a 64-bit signed integer"
                            ),
                        )
                    })?;
                Ok(SignedNumLit::Int(IntBound {
                    value: if neg { -n } else { n },
                    span: span_of(t.span),
                }))
            }
            Some(t) if t.kind == TokenKind::FloatLit => {
                self.bump();
                let slice = self.slice(t.span);
                // tokenize() already validated the separator-free form, so this
                // parse cannot fail — but a silent NaN here became a NaN
                // refinement bound (`InRange(0.0, 1_000.5)` admitted nothing),
                // so fail loudly instead (v0.142).
                let v: f64 = crate::lexer::strip_digit_separators(slice)
                    .parse()
                    .map_err(|_| {
                        CompileError::new(
                            "bynk.lex.float_literal_overflow",
                            t.span,
                            format!("float literal `{slice}` does not parse"),
                        )
                    })?;
                let (value, lexeme) = if neg {
                    (-v, format!("-{slice}"))
                } else {
                    (v, slice.to_string())
                };
                Ok(SignedNumLit::Float(FloatBound {
                    value,
                    lexeme,
                    span: span_of(t.span),
                }))
            }
            Some(t) => Err(CompileError::new(
                "bynk.parse.expected_token",
                t.span,
                format!(
                    "expected a numeric literal {ctx}, found {}",
                    t.kind.describe()
                ),
            )),
            None => Err(CompileError::new(
                "bynk.parse.unexpected_eof",
                self.eof_span(),
                format!("expected a numeric literal {ctx}, found end of file"),
            )),
        }
    }

    /// v0.20a: a type reference — an atom, a parenthesised group, or a
    /// function type. `->` is **right-associative** (`A -> B -> C` is
    /// `A -> (B -> C)`). A parenthesised list is a function-type parameter
    /// list when followed by `->`; a grouping when it holds exactly one type
    /// (`(A -> B)` — unwrapped, so the formatter canonicalises); and the unit
    /// type when empty and arrow-free. The disambiguation is deferred to the
    /// arrow peek, so no extra lookahead is needed.
    pub(crate) fn parse_type_ref(&mut self, ctx: &str) -> Result<TypeRef, CompileError> {
        self.enter_recursion("this type")?;
        let result = self.parse_type_ref_inner(ctx);
        self.depth -= 1;
        result
    }

    fn parse_type_ref_inner(&mut self, ctx: &str) -> Result<TypeRef, CompileError> {
        enum Group {
            Unit(Span),
            Single(TypeRef, Span),
            Multi(Vec<TypeRef>, Span),
        }
        let group = if self.peek_kind() == Some(TokenKind::LParen) {
            let open = self.bump().unwrap();
            if self.peek_kind() == Some(TokenKind::RParen) {
                let close = self.bump().unwrap();
                Group::Unit(open.span.merge(close.span))
            } else {
                let mut items = vec![self.parse_type_ref(ctx)?];
                while self.eat(TokenKind::Comma).is_some() {
                    items.push(self.parse_type_ref(ctx)?);
                }
                let close = self.expect(TokenKind::RParen, "to close the parenthesised type")?;
                let span = open.span.merge(close.span);
                if items.len() == 1 {
                    Group::Single(items.pop().unwrap(), span)
                } else {
                    Group::Multi(items, span)
                }
            }
        } else {
            let t = self.parse_type_atom(ctx)?;
            let span = t.span();
            Group::Single(t, span)
        };
        if self.peek_kind() == Some(TokenKind::Arrow) {
            self.bump();
            // Recursing through parse_type_ref makes `->` right-associative.
            let ret = self.parse_type_ref(ctx)?;
            let (params, start) = match group {
                Group::Unit(s) => (Vec::new(), s),
                Group::Single(t, s) => (vec![t], s),
                Group::Multi(ts, s) => (ts, s),
            };
            let span = start.merge(ret.span());
            return Ok(TypeRef::Fn(params, Box::new(ret), span));
        }
        match group {
            Group::Unit(s) => Ok(TypeRef::Unit(s)),
            Group::Single(t, _) => Ok(t),
            Group::Multi(_, s) => Err(CompileError::new(
                "bynk.parse.expected_token",
                s,
                "expected `->` after a parenthesised parameter list — a bare `(A, B)` is not a type",
            )),
        }
    }

    fn parse_type_atom(&mut self, ctx: &str) -> Result<TypeRef, CompileError> {
        match self.peek() {
            Some(t) => match t.kind {
                TokenKind::Int => {
                    self.bump();
                    Ok(TypeRef::Base(BaseType::Int, t.span))
                }
                TokenKind::String => {
                    self.bump();
                    Ok(TypeRef::Base(BaseType::String, t.span))
                }
                TokenKind::Bool => {
                    self.bump();
                    Ok(TypeRef::Base(BaseType::Bool, t.span))
                }
                TokenKind::Float => {
                    self.bump();
                    Ok(TypeRef::Base(BaseType::Float, t.span))
                }
                TokenKind::Duration => {
                    self.bump();
                    Ok(TypeRef::Base(BaseType::Duration, t.span))
                }
                TokenKind::Instant => {
                    self.bump();
                    Ok(TypeRef::Base(BaseType::Instant, t.span))
                }
                TokenKind::Bytes => {
                    self.bump();
                    Ok(TypeRef::Base(BaseType::Bytes, t.span))
                }
                TokenKind::Result => {
                    self.bump();
                    // Must be followed by `[T, E]`.
                    let lb = self.peek().map(|t| t.kind);
                    if lb != Some(TokenKind::LBracket) {
                        return Err(CompileError::new(
                            "bynk.parse.expected_token",
                            t.span,
                            "the built-in `Result` type requires two type arguments: `Result[T, E]`",
                        )
                        .with_note("`Result` cannot appear without its `[T, E]` parameters"));
                    }
                    self.bump();
                    let arg_t = self.parse_type_ref("as the first `Result` type argument")?;
                    // Check for missing comma — typical user error is `Result[T]`.
                    if self.peek_kind() == Some(TokenKind::RBracket) {
                        let close = self.bump().unwrap();
                        return Err(CompileError::new(
                            "bynk.parse.generic_arg_count",
                            t.span.merge(close.span),
                            "the built-in `Result` type requires two type arguments: `Result[T, E]`",
                        )
                        .with_note("`Result` always has two parameters: the success type and the error type"));
                    }
                    self.expect(TokenKind::Comma, "between the `Result` type arguments")?;
                    let arg_e = self.parse_type_ref("as the second `Result` type argument")?;
                    let close =
                        self.expect(TokenKind::RBracket, "to close the `Result` type arguments")?;
                    Ok(TypeRef::Result(
                        Box::new(arg_t),
                        Box::new(arg_e),
                        t.span.merge(close.span),
                    ))
                }
                TokenKind::ValidationError => {
                    self.bump();
                    Ok(TypeRef::ValidationError(t.span))
                }
                TokenKind::JsonError => {
                    self.bump();
                    Ok(TypeRef::JsonError(t.span))
                }
                TokenKind::Option => {
                    self.bump();
                    if self.peek_kind() != Some(TokenKind::LBracket) {
                        return Err(CompileError::new(
                            "bynk.parse.expected_token",
                            t.span,
                            "the built-in `Option` type requires one type argument: `Option[T]`",
                        ));
                    }
                    self.bump();
                    let arg = self.parse_type_ref("as the `Option` type argument")?;
                    let close =
                        self.expect(TokenKind::RBracket, "to close the `Option` type argument")?;
                    Ok(TypeRef::Option(Box::new(arg), t.span.merge(close.span)))
                }
                TokenKind::Effect => {
                    self.bump();
                    if self.peek_kind() != Some(TokenKind::LBracket) {
                        return Err(CompileError::new(
                            "bynk.parse.expected_token",
                            t.span,
                            "the built-in `Effect` type requires one type argument: `Effect[T]`",
                        ));
                    }
                    self.bump();
                    let arg = self.parse_type_ref("as the `Effect` type argument")?;
                    let close =
                        self.expect(TokenKind::RBracket, "to close the `Effect` type argument")?;
                    Ok(TypeRef::Effect(Box::new(arg), t.span.merge(close.span)))
                }
                TokenKind::Ident => {
                    self.bump();
                    let name = self.slice(t.span).to_string();
                    // Built-in type-name dispatch. Each arm below uses the exact
                    // `name == "<Name>"` idiom: the drift guard
                    // `builtin_type_names_match_parser_dispatch`
                    // (`bynkc/tests/keywords_reference.rs`) scans this file for
                    // that idiom and asserts the set equals `BUILTIN_TYPE_NAMES`,
                    // which also gates the `bynk.resolve.reserved_builtin_type`
                    // redeclaration diagnostic. Keep new built-ins in this form,
                    // and register them there.
                    //
                    // v0.9: `HttpResult` is a predeclared built-in generic.
                    if name == "HttpResult" {
                        if self.peek_kind() != Some(TokenKind::LBracket) {
                            return Err(CompileError::new(
                                "bynk.parse.expected_token",
                                t.span,
                                "the built-in `HttpResult` type requires one type argument: `HttpResult[T]`",
                            ));
                        }
                        self.bump();
                        let arg = self.parse_type_ref("as the `HttpResult` type argument")?;
                        let close = self.expect(
                            TokenKind::RBracket,
                            "to close the `HttpResult` type argument",
                        )?;
                        return Ok(TypeRef::HttpResult(Box::new(arg), t.span.merge(close.span)));
                    }
                    // v0.44: `QueueResult` is a predeclared non-generic built-in.
                    if name == "QueueResult" {
                        return Ok(TypeRef::QueueResult(t.span));
                    }
                    // #1883, slice 1a: `Asset` is a predeclared non-generic
                    // built-in, the type of an `asset` declaration's name.
                    if name == "Asset" {
                        return Ok(TypeRef::Asset(t.span));
                    }
                    // #2341 (decision A): `RequestHeaders` is a predeclared
                    // non-generic built-in, an HTTP handler's `headers`.
                    if name == "RequestHeaders" {
                        return Ok(TypeRef::RequestHeaders(t.span));
                    }
                    // #2380 (L5): an `Oidc` binder's verified claims.
                    if name == "TokenClaims" {
                        return Ok(TypeRef::TokenClaims(t.span));
                    }
                    // #2341 (decision C): the parameter of `on rejected`.
                    if name == "BoundaryRejection" {
                        return Ok(TypeRef::BoundaryRejection(t.span));
                    }
                    // v0.20b: `List` and `Map` are predeclared built-in generics.
                    if name == "List" {
                        if self.peek_kind() != Some(TokenKind::LBracket) {
                            return Err(CompileError::new(
                                "bynk.parse.expected_token",
                                t.span,
                                "the built-in `List` type requires one type argument: `List[T]`",
                            ));
                        }
                        self.bump();
                        let arg = self.parse_type_ref("as the `List` type argument")?;
                        let close =
                            self.expect(TokenKind::RBracket, "to close the `List` type argument")?;
                        return Ok(TypeRef::List(Box::new(arg), t.span.merge(close.span)));
                    }
                    // K6a (#2596): `Set[T]`, the built-in immutable set. Like
                    // `List`, a reserved built-in type name (`keywords.rs`).
                    if name == "Set" {
                        if self.peek_kind() != Some(TokenKind::LBracket) {
                            return Err(CompileError::new(
                                "bynk.parse.expected_token",
                                t.span,
                                "the built-in `Set` type requires one type argument: `Set[T]`",
                            ));
                        }
                        self.bump();
                        let arg = self.parse_type_ref("as the `Set` type argument")?;
                        let close =
                            self.expect(TokenKind::RBracket, "to close the `Set` type argument")?;
                        return Ok(TypeRef::Set(Box::new(arg), t.span.merge(close.span)));
                    }
                    // v0.91 (ADR 0115): `Query[T]` — the lazy storage-read type.
                    if name == "Query" {
                        if self.peek_kind() != Some(TokenKind::LBracket) {
                            return Err(CompileError::new(
                                "bynk.parse.expected_token",
                                t.span,
                                "the built-in `Query` type requires one type argument: `Query[T]`",
                            ));
                        }
                        self.bump();
                        let arg = self.parse_type_ref("as the `Query` type argument")?;
                        let close =
                            self.expect(TokenKind::RBracket, "to close the `Query` type argument")?;
                        return Ok(TypeRef::Query(Box::new(arg), t.span.merge(close.span)));
                    }
                    // v0.100: `Stream[T]` — the value-over-time primitive
                    // (real-time track, slice 0).
                    if name == "Stream" {
                        if self.peek_kind() != Some(TokenKind::LBracket) {
                            return Err(CompileError::new(
                                "bynk.parse.expected_token",
                                t.span,
                                "the built-in `Stream` type requires one type argument: `Stream[T]`",
                            ));
                        }
                        self.bump();
                        let arg = self.parse_type_ref("as the `Stream` type argument")?;
                        let close = self
                            .expect(TokenKind::RBracket, "to close the `Stream` type argument")?;
                        return Ok(TypeRef::Stream(Box::new(arg), t.span.merge(close.span)));
                    }
                    // v0.102: `Connection[F]` — a held WebSocket connection
                    // (real-time track, slice 2). `F` is the server→client frame
                    // type.
                    if name == "Connection" {
                        if self.peek_kind() != Some(TokenKind::LBracket) {
                            return Err(CompileError::new(
                                "bynk.parse.expected_token",
                                t.span,
                                "the built-in `Connection` type requires one type argument: `Connection[F]`",
                            ));
                        }
                        self.bump();
                        let arg = self.parse_type_ref("as the `Connection` frame type")?;
                        let close = self.expect(
                            TokenKind::RBracket,
                            "to close the `Connection` type argument",
                        )?;
                        return Ok(TypeRef::Connection(Box::new(arg), t.span.merge(close.span)));
                    }
                    // v0.119 (testing track slice 7, ADR 0155): `History[Agent]` — a
                    // generated, driven call-history. A test-only generator; the checker
                    // admits it only in `for all` binding position inside a `property`.
                    if name == "History" {
                        if self.peek_kind() != Some(TokenKind::LBracket) {
                            return Err(CompileError::new(
                                "bynk.parse.expected_token",
                                t.span,
                                "the built-in `History` type requires one type argument: `History[Agent]`",
                            ));
                        }
                        self.bump();
                        let arg = self.parse_type_ref("as the `History` type argument")?;
                        let close = self
                            .expect(TokenKind::RBracket, "to close the `History` type argument")?;
                        return Ok(TypeRef::History(Box::new(arg), t.span.merge(close.span)));
                    }
                    if name == "Map" {
                        if self.peek_kind() != Some(TokenKind::LBracket) {
                            return Err(CompileError::new(
                                "bynk.parse.expected_token",
                                t.span,
                                "the built-in `Map` type requires two type arguments: `Map[K, V]`",
                            ));
                        }
                        self.bump();
                        let arg_k = self.parse_type_ref("as the first `Map` type argument")?;
                        if self.peek_kind() == Some(TokenKind::RBracket) {
                            let close = self.bump().unwrap();
                            return Err(CompileError::new(
                                "bynk.parse.generic_arg_count",
                                t.span.merge(close.span),
                                "the built-in `Map` type requires two type arguments: `Map[K, V]`",
                            ));
                        }
                        self.expect(TokenKind::Comma, "between the `Map` type arguments")?;
                        let arg_v = self.parse_type_ref("as the second `Map` type argument")?;
                        let close =
                            self.expect(TokenKind::RBracket, "to close the `Map` type arguments")?;
                        return Ok(TypeRef::Map(
                            Box::new(arg_k),
                            Box::new(arg_v),
                            t.span.merge(close.span),
                        ));
                    }
                    // v0.157 (ADR 0183): `Name[Arg, …]` — application of a
                    // user-declared generic type. In a type position a `[`
                    // following a type name is unambiguously type application
                    // (there are no list literals here), so it is consumed
                    // unconditionally. Existence and arity are the resolver's.
                    if self.peek_kind() == Some(TokenKind::LBracket) {
                        let name_id = Ident { name, span: t.span };
                        self.bump();
                        let mut args = Vec::new();
                        if self.peek_kind() == Some(TokenKind::RBracket) {
                            let close = self.bump().unwrap();
                            return Err(CompileError::new(
                                "bynk.parse.generic_arg_count",
                                t.span.merge(close.span),
                                format!(
                                    "type `{}` is applied to an empty argument list — `{}[…]` needs at least one type argument",
                                    name_id.name, name_id.name
                                ),
                            ));
                        }
                        loop {
                            args.push(self.parse_type_ref("as a type argument")?);
                            if self.eat(TokenKind::Comma).is_none() {
                                break;
                            }
                        }
                        let close =
                            self.expect(TokenKind::RBracket, "to close the type-argument list")?;
                        return Ok(TypeRef::App {
                            name: name_id,
                            args,
                            span: t.span.merge(close.span),
                        });
                    }
                    Ok(TypeRef::Named(Ident { name, span: t.span }))
                }
                _ => Err(CompileError::new(
                    "bynk.parse.expected_type",
                    t.span,
                    format!("expected a type {ctx}, found {}", t.kind.describe()),
                )),
            },
            None => Err(CompileError::new(
                "bynk.parse.unexpected_eof",
                self.eof_span(),
                format!("expected a type {ctx}, found end of file"),
            )),
        }
    }
}

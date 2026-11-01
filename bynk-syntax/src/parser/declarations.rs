//! Declaration parsing — unit/commons/context/test/mock/adapter/binding
//! declarations and their fragment forms. Split out of `parser.rs`
//! (ADR 0060) as a second `impl Parser` block; the scanning core (`expect`,
//! `peek`, `bump`, the trivia/doc helpers) and the other parse concerns stay
//! in the parent module, reached as ancestor privates via `self`.

use super::*;

impl<'a> Parser<'a> {
    pub(crate) fn parse_unit(&mut self) -> Result<SourceUnit, CompileError> {
        // Optional doc block describing the declaration itself, plus any
        // line comments that lead the file.
        let (mut header_leading, leading_doc) = self.collect_item_lead();
        match self.peek_kind() {
            Some(TokenKind::Commons) => {
                let start = self.expect(TokenKind::Commons, "to start the commons declaration")?;
                let doc = self.finalize_doc(leading_doc, start.span, &mut header_leading);
                let name = self.parse_qualified_name()?;
                let mut c = match self.peek_kind() {
                    Some(TokenKind::LBrace) => {
                        self.parse_commons_body(start.span, name, doc, true)?
                    }
                    _ => self.parse_commons_body(start.span, name, doc, false)?,
                };
                c.trivia = Trivia {
                    leading: header_leading,
                    trailing: None,
                };
                Ok(SourceUnit::Commons(c))
            }
            Some(TokenKind::Context) => {
                let start = self.expect(TokenKind::Context, "to start the context declaration")?;
                let doc = self.finalize_doc(leading_doc, start.span, &mut header_leading);
                let name = self.parse_qualified_name()?;
                let mut c = match self.peek_kind() {
                    Some(TokenKind::LBrace) => {
                        self.parse_context_body(start.span, name, doc, true)?
                    }
                    _ => self.parse_context_body(start.span, name, doc, false)?,
                };
                c.trivia = Trivia {
                    leading: header_leading,
                    trailing: None,
                };
                Ok(SourceUnit::Context(c))
            }
            Some(TokenKind::Adapter) => {
                let start = self.expect(TokenKind::Adapter, "to start the adapter declaration")?;
                let doc = self.finalize_doc(leading_doc, start.span, &mut header_leading);
                let name = self.parse_qualified_name()?;
                let mut a = match self.peek_kind() {
                    Some(TokenKind::LBrace) => {
                        self.parse_adapter_body(start.span, name, doc, true)?
                    }
                    _ => self.parse_adapter_body(start.span, name, doc, false)?,
                };
                a.trivia = Trivia {
                    leading: header_leading,
                    trailing: None,
                };
                Ok(SourceUnit::Adapter(a))
            }
            Some(TokenKind::Suite) => {
                let start = self.expect(TokenKind::Suite, "to start the suite declaration")?;
                let doc = self.finalize_doc(leading_doc, start.span, &mut header_leading);
                let name = self.parse_qualified_name()?;
                // v0.118: an optional `as <tier>` sets the suite's default tier,
                // which its `case` members inherit and override (a `property`
                // ignores it). `as` is unambiguous here — no `consumes` is in
                // scope in a suite header (DECISION N).
                let tier = if self.peek_kind() == Some(TokenKind::As) {
                    self.bump();
                    Some(self.parse_tier()?)
                } else {
                    None
                };
                let mut t = match self.peek_kind() {
                    Some(TokenKind::LBrace) => {
                        self.parse_test_body(start.span, name, doc, tier, true)?
                    }
                    _ => self.parse_test_body(start.span, name, doc, tier, false)?,
                };
                t.trivia = Trivia {
                    leading: header_leading,
                    trailing: None,
                };
                Ok(SourceUnit::Suite(t))
            }
            Some(_) => {
                let t = self.peek().unwrap();
                if let Some(DocLead { span: doc_span, .. }) = leading_doc {
                    self.warnings.push(CompileError::new(
                        "bynk.parse.orphan_doc_block",
                        doc_span,
                        "documentation block has no following declaration to attach to",
                    ));
                }
                Err(CompileError::new(
                    "bynk.parse.expected_unit_header",
                    t.span,
                    format!(
                        "expected `commons`, `context`, or `suite` to start the file, found {}",
                        t.kind.describe()
                    ),
                )
                .with_note(
                    "every `.bynk` file begins with either a `commons`, `context`, or `suite` declaration",
                ))
            }
            None => {
                if let Some(DocLead { span: doc_span, .. }) = leading_doc {
                    self.warnings.push(CompileError::new(
                        "bynk.parse.orphan_doc_block",
                        doc_span,
                        "documentation block has no following declaration to attach to",
                    ));
                }
                Err(CompileError::new(
                    "bynk.parse.unexpected_eof",
                    self.eof_span(),
                    "expected `commons`, `context`, or `suite` to start the file, found end of file",
                ))
            }
        }
    }

    /// Parse a commons body, brace or fragment form (`brace` selects which).
    /// Unified from two near-duplicate functions (review Part 3, "the
    /// six-fold item loop") behind the same `brace: bool` parameter
    /// [`Self::parse_adapter_body`] already used — the fragment-only
    /// bookkeeping (`last_span`, `seen_item`, `uses_after_decls`) is simply
    /// unconditional now: it's harmless dead weight in brace form (whose own
    /// `end.span` wins over `last_span`, and whose `uses` may interleave with
    /// other items in either case — `seen_item`'s check is itself gated on
    /// `!brace`).
    fn parse_commons_body(
        &mut self,
        start: Span,
        name: QualifiedName,
        documentation: Option<Doc>,
        brace: bool,
    ) -> Result<Commons, CompileError> {
        if brace {
            self.expect(TokenKind::LBrace, "after the commons name")?;
        }
        self.enter_item_loop();
        let mut items = Vec::new();
        let mut uses = Vec::new();
        // Cover the header (`commons <name>`) so the unit span stays valid even
        // when every item is dropped by error recovery — the document-symbol
        // selection range (the name span) must remain contained in this span.
        // Only consulted in fragment form; brace form's span comes from its
        // own closing `}` instead.
        let mut last_span = start.merge(name.span);
        let mut seen_item = false;
        let trailing_comments: Vec<Comment>;
        loop {
            // Optional doc block and leading line comments before the next item.
            let (mut leading, item_doc) = self.collect_item_lead();
            self.item_start = Some(self.pos);
            let private = self.take_private_modifier(true)?;
            match self.peek_kind() {
                Some(TokenKind::RBrace) if brace => {
                    // Doc not attachable; treat as orphan if present. Any
                    // leading comments at this position end up as the
                    // body's trailing comments.
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following declaration to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    trailing_comments = std::mem::take(&mut leading);
                    break;
                }
                None if !brace => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following declaration to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    // Comments we held as leading for the next item, plus
                    // any held in the trivia table's epilogue, become the
                    // commons body's trailing comments.
                    leading.extend(self.take_epilogue_trivia());
                    trailing_comments = leading;
                    break;
                }
                Some(TokenKind::Uses) => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(
                            CompileError::new(
                                "bynk.parse.orphan_doc_block",
                                doc_span,
                                "documentation block before `uses` is not allowed; only `type` and `fn` declarations carry docs",
                            ),
                        );
                        keep_orphan(&mut leading, doc);
                    }
                    if !brace && seen_item {
                        let t = self.peek().unwrap();
                        return Err(CompileError::new(
                            "bynk.parse.uses_after_decls",
                            t.span,
                            "`uses` clauses must appear before any `type` or `fn` declaration in a fragment-form commons",
                        )
                        .with_note(
                            "move all `uses` lines to immediately after the `commons` header",
                        ));
                    }
                    match self.parse_uses_decl() {
                        Ok(mut u) => {
                            u.trivia.leading = leading;
                            u.trivia.trailing = self.take_trailing_trivia();
                            last_span = u.span;
                            uses.push(u);
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Type) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_type_decl() {
                        Ok(mut t) => {
                            t.documentation = doc;
                            if let Some(sp) = private {
                                t.private = Some(sp);
                                t.span = sp.merge(t.span);
                            }
                            t.trivia.leading = leading;
                            t.trivia.trailing = self.take_trailing_trivia();
                            last_span = t.span;
                            items.push(CommonsItem::Type(t));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Fn) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_fn_decl() {
                        Ok(mut f) => {
                            f.documentation = doc;
                            if let Some(sp) = private {
                                f.private = Some(sp);
                                f.span = sp.merge(f.span);
                            }
                            f.trivia.leading = leading;
                            f.trivia.trailing = self.take_trailing_trivia();
                            last_span = f.span;
                            items.push(CommonsItem::Fn(f));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Messages) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_messages_decl() {
                        Ok(mut m) => {
                            m.documentation = doc;
                            if let Some(sp) = private {
                                m.private = Some(sp);
                                m.span = sp.merge(m.span);
                            }
                            m.trivia.leading = leading;
                            m.trivia.trailing = self.take_trailing_trivia();
                            last_span = m.span;
                            items.push(CommonsItem::Messages(m));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // Events track, slice 0 (spine #936): `event` parses into
                // items too, so the checker can reject it precisely
                // (`bynk.event.outside_context`) — same reasoning as
                // `messages` above, mirrored to the opposite placement.
                Some(TokenKind::Event) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_event_decl() {
                        Ok(mut e) => {
                            e.documentation = doc;
                            e.trivia.leading = leading;
                            e.trivia.trailing = self.take_trailing_trivia();
                            last_span = e.span;
                            items.push(CommonsItem::Event(e));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #1883, slice 1a: `asset` parses wherever any item does;
                // placement (context or commons, never an adapter) is the
                // checker's call (`bynk.asset.outside_context_or_commons`),
                // the same split as `event`/`messages` above.
                Some(TokenKind::Asset) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_asset_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            if let Some(sp) = private {
                                a.private = Some(sp);
                                a.span = sp.merge(a.span);
                            }
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Asset(a));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #2387, slice 1: `embed` parses wherever any item does;
                // placement (context or commons, never an adapter) is the
                // checker's call (`bynk.embed.outside_context_or_commons`),
                // the same split as `event`/`messages` above.
                Some(TokenKind::Embed) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_embed_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            if let Some(sp) = private {
                                a.private = Some(sp);
                                a.span = sp.merge(a.span);
                            }
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Embed(a));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #2617: `const` parses wherever `embed` does; placement
                // (context or commons, never an adapter) is the checker's call
                // (`bynk.resolve.const_outside_context_or_commons`).
                Some(TokenKind::Const) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_const_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Const(a));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Capability) => {
                    let err = CompileError::new(
                        "bynk.capability.outside_context",
                        self.peek().unwrap().span,
                        "`capability` declarations are only allowed inside a context, not a commons",
                    );
                    self.handle_item_err(err)?;
                }
                Some(TokenKind::Provides) => {
                    let err = CompileError::new(
                        "bynk.provider.outside_context",
                        self.peek().unwrap().span,
                        "`provides` declarations are only allowed inside a context, not a commons",
                    );
                    self.handle_item_err(err)?;
                }
                Some(TokenKind::Service) => {
                    let err = CompileError::new(
                        "bynk.service.outside_context",
                        self.peek().unwrap().span,
                        "`service` declarations are only allowed inside a context, not a commons",
                    );
                    self.handle_item_err(err)?;
                }
                Some(TokenKind::Agent) => {
                    let err = CompileError::new(
                        "bynk.agent.outside_context",
                        self.peek().unwrap().span,
                        "`agent` declarations are only allowed inside a context, not a commons",
                    );
                    self.handle_item_err(err)?;
                }
                Some(TokenKind::Actor) => {
                    let err = CompileError::new(
                        "bynk.actor.outside_context",
                        self.peek().unwrap().span,
                        "`actor` declarations are only allowed inside a context, not a commons",
                    );
                    self.handle_item_err(err)?;
                }
                _ => {
                    let t = match self.peek() {
                        Some(t) => t,
                        None => {
                            return Err(CompileError::new(
                                "bynk.parse.unexpected_eof",
                                self.eof_span(),
                                "expected `}` to close the commons body, found end of file",
                            ));
                        }
                    };
                    let note = if brace {
                        "the body of a commons contains zero or more `type`, `fn`, `messages`, or `uses` declarations"
                    } else {
                        "in fragment-form commons (no braces), the body is a sequence of `type`, `fn`, `messages`, or `uses` declarations to end of file"
                    };
                    let err = CompileError::new(
                        "bynk.parse.expected_item",
                        t.span,
                        format!(
                            "expected `type`, `fn`, `messages`, or `uses` declaration, found {}",
                            t.kind.describe()
                        ),
                    )
                    .with_note(note);
                    if self.recover_mode {
                        self.recovered_errors.push(err);
                        // Force progress in recovery: bump at least one token, then sync.
                        self.bump();
                        self.recover_to_top_item();
                    } else {
                        return Err(err);
                    }
                }
            }
        }
        let mut trailing_comments = trailing_comments;
        let span = if brace {
            let end = self.expect(TokenKind::RBrace, "to close the commons body")?;
            self.exit_item_loop();
            // A comment after the closing `}` (this being the last
            // declaration in the file) is a no-op `take_epilogue` unless it
            // truly is the last content in the file — see the fragment-form
            // break arm's matching call above.
            trailing_comments.extend(self.take_epilogue_trivia());
            start.merge(end.span)
        } else {
            self.exit_item_loop();
            start.merge(last_span)
        };
        Ok(Commons {
            name,
            items,
            uses,
            documentation,
            form: if brace {
                CommonsForm::Brace
            } else {
                CommonsForm::Fragment
            },
            span,
            trivia: Trivia::default(),
            trailing_comments,
        })
    }

    fn parse_uses_decl(&mut self) -> Result<UsesDecl, CompileError> {
        let kw = self.expect(TokenKind::Uses, "to start a `uses` declaration")?;
        let target = self.parse_qualified_name()?;
        let span = kw.span.merge(target.span);
        Ok(UsesDecl {
            target,
            span,
            trivia: Trivia::default(),
        })
    }

    fn parse_consumes_decl(&mut self) -> Result<ConsumesDecl, CompileError> {
        let kw = self.expect(TokenKind::Consumes, "to start a `consumes` declaration")?;
        let target = self.parse_qualified_name()?;
        let mut span = kw.span.merge(target.span);
        let mut alias = None;
        let mut selected = None;
        match self.peek_kind() {
            // v0.6: `consumes U as Alias`.
            Some(TokenKind::As) => {
                self.bump();
                let id = self.expect_ident("as an alias for the consumed context")?;
                span = span.merge(id.span);
                alias = Some(id);
            }
            // v0.17: `consumes U { Cap, … }` — selected capabilities (§3.3).
            Some(TokenKind::LBrace) => {
                self.bump();
                let mut names = Vec::new();
                while self.peek_kind() != Some(TokenKind::RBrace) {
                    let id = self.expect_ident("a capability name in `consumes U { … }`")?;
                    names.push(id);
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                }
                let close =
                    self.expect(TokenKind::RBrace, "to close the consumed-capability list")?;
                span = span.merge(close.span);
                selected = Some(names);
            }
            _ => {}
        }
        Ok(ConsumesDecl {
            target,
            alias,
            selected,
            span,
            trivia: Trivia::default(),
        })
    }

    fn parse_exports_decl(&mut self) -> Result<ExportsDecl, CompileError> {
        let kw = self.expect(TokenKind::Exports, "to start an `exports` declaration")?;
        let kind = match self.peek_kind() {
            Some(TokenKind::Opaque) => {
                self.bump();
                ExportKind::Type(Visibility::Opaque)
            }
            Some(TokenKind::Transparent) => {
                self.bump();
                ExportKind::Type(Visibility::Transparent)
            }
            // v0.15: `exports capability { ... }` offers capabilities to consumers.
            Some(TokenKind::Capability) => {
                self.bump();
                ExportKind::Capability
            }
            Some(_) => {
                let t = self.peek().unwrap();
                return Err(CompileError::new(
                    "bynk.parse.expected_visibility",
                    t.span,
                    format!(
                        "expected `opaque`, `transparent`, or `capability` after `exports`, found {}",
                        t.kind.describe()
                    ),
                )
                .with_note(
                    "exports clauses are `exports opaque { ... }`, `exports transparent { ... }`, or `exports capability { ... }`",
                ));
            }
            None => {
                return Err(CompileError::new(
                    "bynk.parse.unexpected_eof",
                    self.eof_span(),
                    "expected `opaque`, `transparent`, or `capability` after `exports`, found end of file",
                ));
            }
        };
        self.expect(TokenKind::LBrace, "to open the exports list")?;
        let mut names = Vec::new();
        let name_role = match kind {
            ExportKind::Capability => "as an exported capability name",
            ExportKind::Type(_) => "as an exported type name",
        };
        while self.peek_kind() != Some(TokenKind::RBrace) {
            // #1797: a name's end-of-line comment sits after its `,`.
            let leading = self.take_leading_trivia();
            let name = self.expect_ident(name_role)?;
            let comma = self.eat(TokenKind::Comma);
            names.push(ExportName {
                name,
                trivia: Trivia {
                    leading,
                    trailing: self.take_trailing_trivia(),
                },
            });
            if comma.is_none() {
                break;
            }
        }
        let trailing_comments = self.take_leading_trivia();
        let close = self.expect(TokenKind::RBrace, "to close the exports list")?;
        let span = kw.span.merge(close.span);
        Ok(ExportsDecl {
            kind,
            names,
            span,
            trivia: Trivia::default(),
            trailing_comments,
        })
    }

    /// Parse a suite/test body, brace or fragment form — see
    /// [`Self::parse_commons_body`]. The `uses_after_decls` error here
    /// carries no `.with_note(...)` (unlike its commons/context
    /// counterparts) — a pre-existing asymmetry, carried over unchanged.
    fn parse_test_body(
        &mut self,
        start: Span,
        target: QualifiedName,
        documentation: Option<Doc>,
        tier: Option<TestTier>,
        brace: bool,
    ) -> Result<SuiteDecl, CompileError> {
        if brace {
            self.expect(TokenKind::LBrace, "after the test target name")?;
        }
        self.enter_item_loop();
        let mut uses = Vec::new();
        let mut stubs = Vec::new();
        let mut envs = Vec::new();
        let mut cases = Vec::new();
        let mut properties = Vec::new();
        // Cover the header (`test <target>`) so the unit span stays valid even
        // when every item is dropped by error recovery. Only consulted in
        // fragment form (see `parse_commons_body`).
        let mut last_span = start.merge(target.span);
        let mut seen_non_uses = false;
        let trailing_comments: Vec<Comment>;
        loop {
            let (mut leading, item_doc) = self.collect_item_lead();
            self.item_start = Some(self.pos);
            match self.peek_kind() {
                Some(TokenKind::RBrace) if brace => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following declaration to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    trailing_comments = std::mem::take(&mut leading);
                    break;
                }
                None if !brace => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following declaration to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    leading.extend(self.take_epilogue_trivia());
                    trailing_comments = leading;
                    break;
                }
                Some(TokenKind::Uses) => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block before `uses` is not allowed",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    if !brace && seen_non_uses {
                        let t = self.peek().unwrap();
                        return Err(CompileError::new(
                            "bynk.parse.uses_after_decls",
                            t.span,
                            "`uses` clauses must appear before any `stub`, `env`, `case`, or `property` declarations in a fragment-form test",
                        ));
                    }
                    match self.parse_uses_decl() {
                        Ok(mut u) => {
                            u.trivia.leading = leading;
                            u.trivia.trailing = self.take_trailing_trivia();
                            last_span = u.span;
                            uses.push(u);
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Stub) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_stub_clause() {
                        Ok(mut p) => {
                            p.documentation = doc;
                            p.trivia.leading = leading;
                            p.trivia.trailing = self.take_trailing_trivia();
                            last_span = p.span;
                            stubs.push(p);
                            seen_non_uses = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #2134: `env { … }` — `env` is contextual, a keyword only
                // here (and at the head of a case body).
                Some(TokenKind::Ident) if self.at_env_clause() => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_env_clause() {
                        Ok(mut e) => {
                            e.documentation = doc;
                            e.trivia.leading = leading;
                            e.trivia.trailing = self.take_trailing_trivia();
                            last_span = e.span;
                            envs.push(e);
                            seen_non_uses = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Case) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_test_case() {
                        Ok(mut c) => {
                            c.documentation = doc;
                            c.trivia.leading = leading;
                            c.trivia.trailing = self.take_trailing_trivia();
                            last_span = c.span;
                            cases.push(c);
                            seen_non_uses = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Property) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_property() {
                        Ok(mut p) => {
                            p.documentation = doc;
                            p.trivia.leading = leading;
                            p.trivia.trailing = self.take_trailing_trivia();
                            last_span = p.span;
                            properties.push(p);
                            seen_non_uses = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                _ => {
                    let t = match self.peek() {
                        Some(t) => t,
                        None => {
                            return Err(CompileError::new(
                                "bynk.parse.unexpected_eof",
                                self.eof_span(),
                                "expected `}` to close the test body, found end of file",
                            ));
                        }
                    };
                    let note = if brace {
                        "the body of a suite contains zero or more `uses`, `stub`, `env`, `case`, or `property` declarations"
                    } else {
                        "in fragment-form suites, the body is a sequence of `uses`, `stub`, `env`, `case`, or `property` declarations to end of file"
                    };
                    let err = CompileError::new(
                        "bynk.parse.expected_item",
                        t.span,
                        format!(
                            "expected `uses`, `stub`, `env`, `case \"name\"`, or `property \"name\"` declaration, found {}",
                            t.kind.describe()
                        ),
                    )
                    .with_note(note);
                    if self.recover_mode {
                        self.recovered_errors.push(err);
                        self.bump();
                        self.recover_to_top_item();
                    } else {
                        return Err(err);
                    }
                }
            }
        }
        let mut trailing_comments = trailing_comments;
        let span = if brace {
            let end = self.expect(TokenKind::RBrace, "to close the test body")?;
            self.exit_item_loop();
            // See `parse_commons_body`'s matching call.
            trailing_comments.extend(self.take_epilogue_trivia());
            start.merge(end.span)
        } else {
            self.exit_item_loop();
            start.merge(last_span)
        };
        Ok(SuiteDecl {
            target,
            uses,
            stubs,
            envs,
            cases,
            properties,
            tier,
            form: if brace {
                CommonsForm::Brace
            } else {
                CommonsForm::Fragment
            },
            documentation,
            span,
            trivia: Trivia::default(),
            trailing_comments,
        })
    }

    /// v0.118: parse a tier name (`unit` / `integration` / `system`) after `as`.
    /// The names are contextual identifiers, not keywords, so they stay usable
    /// as ordinary identifiers everywhere else.
    fn parse_tier(&mut self) -> Result<TestTier, CompileError> {
        let tok = self.expect(TokenKind::Ident, "a tier name after `as`")?;
        match self.slice(tok.span) {
            "unit" => Ok(TestTier::Unit),
            "integration" => Ok(TestTier::Integration),
            "system" => Ok(TestTier::System),
            other => Err(CompileError::new(
                "bynk.parse.unknown_tier",
                tok.span,
                format!(
                    "`{other}` is not a test tier; expected `unit`, `integration`, or `system`"
                ),
            )
            .with_note(
                "a `case`/`suite` tier clause is `as unit`, `as integration`, or `as system`",
            )),
        }
    }

    /// v0.118: parse a `stub Cap.method(<args>) returns <v> | fails` clause
    /// (the leading `stub` has not yet been consumed). Keyword `stub` since the
    /// keyword-hygiene batch (#548); formerly a pun on `provides`.
    fn parse_stub_clause(&mut self) -> Result<StubClause, CompileError> {
        let kw = self.expect(TokenKind::Stub, "to start a `stub` clause")?;
        let capability = self.expect_ident("the capability after `stub`")?;
        self.expect(TokenKind::Dot, "after the capability in a `stub` clause")?;
        let method = self.expect_ident("the method name after `.`")?;
        self.expect(
            TokenKind::LParen,
            "after the method name in a `stub` clause",
        )?;
        let mut args = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            args.push(self.parse_arg_pattern()?);
            while self.eat(TokenKind::Comma).is_some() {
                if self.peek_kind() == Some(TokenKind::RParen) {
                    break;
                }
                args.push(self.parse_arg_pattern()?);
            }
        }
        self.expect(TokenKind::RParen, "to close the `stub` argument list")?;
        let rhs = self.parse_stub_rhs()?;
        // #2683: `returns <v> after <d>`. `after` is a contextual word; an
        // expression is never followed by a bare identifier, so seeing one
        // here is unambiguous.
        let after = if matches!(rhs, StubRhs::Returns(_))
            && self.peek_kind() == Some(TokenKind::Ident)
            && self.nth_text(0) == "after"
        {
            self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        let span = kw
            .span
            .merge(after.as_ref().map_or_else(|| rhs.span(), |e| e.span));
        Ok(StubClause {
            capability,
            method,
            args,
            rhs,
            after,
            documentation: None,
            span,
            trivia: Trivia::default(),
        })
    }

    /// #2134: whether the cursor is at an `env {` clause head. `env` is a
    /// contextual word, so it is recognised only when a `{` follows it.
    fn at_env_clause(&self) -> bool {
        self.peek_kind() == Some(TokenKind::Ident)
            && self.nth_text(0) == "env"
            && self.nth_kind(1) == Some(TokenKind::LBrace)
    }

    /// #2134: parse an `env { NAME = <value> … }` clause (the leading `env` has
    /// not yet been consumed). Entries are separated by newlines (or commas);
    /// each value is an expression, which the checker requires to be a
    /// `String` literal.
    fn parse_env_clause(&mut self) -> Result<EnvClause, CompileError> {
        let kw = self.expect(TokenKind::Ident, "to start an `env` clause")?;
        self.expect(TokenKind::LBrace, "after `env`")?;
        let mut entries = Vec::new();
        while self.peek_kind() != Some(TokenKind::RBrace) {
            if self.peek_kind().is_none() {
                return Err(CompileError::new(
                    "bynk.parse.unexpected_eof",
                    self.eof_span(),
                    "expected `}` to close the `env` clause, found end of file",
                ));
            }
            let name = self.expect_ident("a variable name in an `env` clause")?;
            self.expect(TokenKind::Eq, "after the variable name in an `env` clause")?;
            let value = self.parse_expr()?;
            let span = name.span.merge(value.span);
            entries.push(EnvEntry { name, value, span });
            self.eat(TokenKind::Comma);
        }
        let close = self.expect(TokenKind::RBrace, "to close the `env` clause")?;
        Ok(EnvClause {
            entries,
            documentation: None,
            span: kw.span.merge(close.span),
            trivia: Trivia::default(),
        })
    }

    /// One argument pattern in a `stub` call pattern: `_` or a value.
    fn parse_arg_pattern(&mut self) -> Result<ArgPattern, CompileError> {
        if self.peek_kind() == Some(TokenKind::Underscore) {
            let span = self.peek().unwrap().span;
            self.bump();
            Ok(ArgPattern::Any(span))
        } else {
            Ok(ArgPattern::Value(self.parse_expr()?))
        }
    }

    /// The right-hand side of a `stub` clause: `fails`, `returns <value>`,
    /// or `returns each [<outcome>, …]`. `returns`, `fails`, and `each` are
    /// contextual identifiers here.
    fn parse_stub_rhs(&mut self) -> Result<StubRhs, CompileError> {
        let tok = self.expect(
            TokenKind::Ident,
            "`returns`, `fails` or `never` after the `stub` call pattern",
        )?;
        match self.slice(tok.span) {
            "fails" => Ok(StubRhs::Fails(tok.span)),
            "never" => Ok(StubRhs::Never(tok.span)),
            "returns" => {
                let is_each = self.peek_kind() == Some(TokenKind::Ident)
                    && self.slice(self.peek().unwrap().span) == "each";
                if is_each {
                    self.bump();
                    let open = self.expect(TokenKind::LBracket, "after `returns each`")?;
                    let mut outcomes = Vec::new();
                    if self.peek_kind() != Some(TokenKind::RBracket) {
                        outcomes.push(self.parse_seq_outcome()?);
                        while self.eat(TokenKind::Comma).is_some() {
                            if self.peek_kind() == Some(TokenKind::RBracket) {
                                break;
                            }
                            outcomes.push(self.parse_seq_outcome()?);
                        }
                    }
                    let close =
                        self.expect(TokenKind::RBracket, "to close the `returns each` sequence")?;
                    Ok(StubRhs::ReturnsEach(outcomes, open.span.merge(close.span)))
                } else {
                    Ok(StubRhs::Returns(self.parse_expr()?))
                }
            }
            other => Err(CompileError::new(
                "bynk.parse.expected_token",
                tok.span,
                format!("expected `returns`, `fails` or `never` in a `stub` clause, found `{other}`"),
            )
            .with_note(
                "a `stub` clause ends `returns <value>` (optionally `after <duration>`), `returns each [<outcome>, …]`, `fails`, or `never`",
            )),
        }
    }

    /// One outcome in a `returns each` sequence: `fails` or a value.
    fn parse_seq_outcome(&mut self) -> Result<SeqOutcome, CompileError> {
        if self.peek_kind() == Some(TokenKind::Ident)
            && self.slice(self.peek().unwrap().span) == "fails"
        {
            let span = self.peek().unwrap().span;
            self.bump();
            Ok(SeqOutcome::Fails(span))
        } else {
            Ok(SeqOutcome::Value(self.parse_expr()?))
        }
    }

    fn parse_test_case(&mut self) -> Result<Case, CompileError> {
        let kw = self.expect(TokenKind::Case, "to start a test case")?;
        let name_tok = self.expect(TokenKind::StrLit, "as the test case name")?;
        let name = parse_string_literal(self.slice(name_tok.span), name_tok.span)?;
        // v0.118: an optional `as <tier>` after the case name.
        let tier = if self.peek_kind() == Some(TokenKind::As) {
            self.bump();
            Some(self.parse_tier()?)
        } else {
            None
        };
        // v0.118: case-scoped `stub` clauses lead the case body, before any
        // statements. The case opens its own brace so it can peel them off.
        let open = self.expect(TokenKind::LBrace, "to open the test case body")?;
        let mut stubs = Vec::new();
        // #2134: and case-scoped `env { … }` clauses, interleaved with them.
        let mut envs = Vec::new();
        while self.peek_kind() == Some(TokenKind::Stub) || self.at_env_clause() {
            let (mut leading, item_doc) = self.collect_body_item_lead();
            let next_span = self.peek().unwrap().span;
            let doc = self.finalize_doc(item_doc, next_span, &mut leading);
            if self.peek_kind() == Some(TokenKind::Stub) {
                let mut p = self.parse_stub_clause()?;
                p.documentation = doc;
                p.trivia.leading = leading;
                p.trivia.trailing = self.take_trailing_trivia();
                stubs.push(p);
            } else {
                let mut e = self.parse_env_clause()?;
                e.documentation = doc;
                e.trivia.leading = leading;
                e.trivia.trailing = self.take_trailing_trivia();
                envs.push(e);
            }
        }
        let body = self.parse_block_rest(open.span)?;
        let span = kw.span.merge(body.span);
        Ok(Case {
            name,
            name_span: name_tok.span,
            tier,
            stubs,
            envs,
            body,
            documentation: None,
            span,
            trivia: Trivia::default(),
        })
    }

    /// v0.114 (testing track slice 2): a generative `property "name" { for all
    /// … }` block inside a suite.
    fn parse_property(&mut self) -> Result<PropertyDecl, CompileError> {
        let kw = self.expect(TokenKind::Property, "to start a property")?;
        let name_tok = self.expect(TokenKind::StrLit, "as the property name")?;
        let name = parse_string_literal(self.slice(name_tok.span), name_tok.span)?;
        self.expect(TokenKind::LBrace, "to open the property body")?;
        let forall = self.parse_for_all()?;
        let end = self.expect(TokenKind::RBrace, "to close the property body")?;
        Ok(PropertyDecl {
            name,
            name_span: name_tok.span,
            forall,
            documentation: None,
            span: kw.span.merge(end.span),
            trivia: Trivia::default(),
        })
    }

    /// `for all x: T, y: U [where <pred>] { … }` — the generative binder. Each
    /// binding's type supplies the runner's inhabitant space; `where` filters
    /// generated tuples before the body runs.
    fn parse_for_all(&mut self) -> Result<ForAll, CompileError> {
        // `for` and `all` are contextual identifiers, not keywords, so `all`
        // stays usable as a list combinator (`all(xs, p)`). Here they lead the
        // binder — validated by text.
        let start = self.expect(TokenKind::Ident, "to start a `for all` binder")?;
        if self.slice(start.span) != "for" {
            return Err(CompileError::new(
                "bynk.parse.expected_token",
                start.span,
                format!(
                    "expected `for` to start a `for all` binder, found `{}`",
                    self.slice(start.span)
                ),
            ));
        }
        let all_tok = self.expect(TokenKind::Ident, "after `for` in a `for all` binder")?;
        if self.slice(all_tok.span) != "all" {
            return Err(CompileError::new(
                "bynk.parse.expected_token",
                all_tok.span,
                format!(
                    "expected `all` after `for` in a `for all` binder, found `{}`",
                    self.slice(all_tok.span)
                ),
            ));
        }
        let mut bindings = Vec::new();
        loop {
            let name = self.expect_ident("as a `for all` binding name")?;
            self.expect(TokenKind::Colon, "after a `for all` binding name")?;
            let type_ref = self.parse_type_ref("as the type of a `for all` binding")?;
            bindings.push(ForAllBinding { name, type_ref });
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        let where_pred = if self.eat(TokenKind::Where).is_some() {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let body = self.parse_block("to open the `for all` body")?;
        let span = start.span.merge(body.span);
        Ok(ForAll {
            bindings,
            where_pred,
            body,
            span,
        })
    }

    /// Parse a context body, brace or fragment form — see
    /// [`Self::parse_commons_body`], the same unification for its sibling.
    /// Fragment form additionally orders `uses`/`consumes`/`exports` before
    /// any declaration (three separate checks, since a context has all
    /// three clause kinds where a commons has only `uses`); each is gated on
    /// `!brace` the same way. `consumes`/`exports`-after-decls recover with
    /// an explicit `continue` (retrying the loop) where `uses`-after-decls
    /// hard-bails even in recovery mode — a pre-existing asymmetry, carried
    /// over unchanged rather than "fixed" as part of this unification.
    fn parse_context_body(
        &mut self,
        start: Span,
        name: QualifiedName,
        documentation: Option<Doc>,
        brace: bool,
    ) -> Result<Context, CompileError> {
        if brace {
            self.expect(TokenKind::LBrace, "after the context name")?;
        }
        self.enter_item_loop();
        let mut items = Vec::new();
        let mut removed_agents: Vec<RemovedAgentDecl> = Vec::new();
        let mut uses = Vec::new();
        let mut consumes = Vec::new();
        let mut exports = Vec::new();
        // Cover the header (`context <name>`) so the unit span stays valid even
        // when every item is dropped by error recovery. Only consulted in
        // fragment form (see `parse_commons_body`).
        let mut last_span = start.merge(name.span);
        let mut seen_item = false;
        let trailing_comments: Vec<Comment>;
        loop {
            let (mut leading, item_doc) = self.collect_item_lead();
            self.item_start = Some(self.pos);
            let private = self.take_private_modifier(true)?;
            match self.peek_kind() {
                Some(TokenKind::RBrace) if brace => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following declaration to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    trailing_comments = std::mem::take(&mut leading);
                    break;
                }
                None if !brace => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following declaration to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    leading.extend(self.take_epilogue_trivia());
                    trailing_comments = leading;
                    break;
                }
                Some(TokenKind::Uses) => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block before `uses` is not allowed; only `type` and `fn` declarations carry docs",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    if !brace && seen_item {
                        let t = self.peek().unwrap();
                        return Err(CompileError::new(
                            "bynk.parse.uses_after_decls",
                            t.span,
                            "`uses` clauses must appear before any `type` or `fn` declaration in a fragment-form context",
                        )
                        .with_note(
                            "move all `uses` lines to immediately after the `context` header",
                        ));
                    }
                    match self.parse_uses_decl() {
                        Ok(mut u) => {
                            u.trivia.leading = leading;
                            u.trivia.trailing = self.take_trailing_trivia();
                            last_span = u.span;
                            uses.push(u);
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Consumes) => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block before `consumes` is not allowed; only `type` and `fn` declarations carry docs",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    if !brace && seen_item {
                        let t = self.peek().unwrap();
                        let err = CompileError::new(
                            "bynk.parse.consumes_after_decls",
                            t.span,
                            "`consumes` clauses must appear before any `type` or `fn` declaration in a fragment-form context",
                        )
                        .with_note(
                            "move all `consumes` lines to immediately after the `uses` clauses",
                        );
                        if self.recover_mode {
                            self.recovered_errors.push(err);
                            self.bump();
                            self.recover_to_top_item();
                            continue;
                        } else {
                            return Err(err);
                        }
                    }
                    match self.parse_consumes_decl() {
                        Ok(mut c) => {
                            c.trivia.leading = leading;
                            c.trivia.trailing = self.take_trailing_trivia();
                            last_span = c.span;
                            consumes.push(c);
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Exports) => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block before `exports` is not allowed; only `type` and `fn` declarations carry docs",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    if !brace && seen_item {
                        let t = self.peek().unwrap();
                        let err = CompileError::new(
                            "bynk.parse.exports_after_decls",
                            t.span,
                            "`exports` clauses must appear before any `type` or `fn` declaration in a fragment-form context",
                        )
                        .with_note(
                            "move all `exports` lines to immediately after the `consumes` clauses",
                        );
                        if self.recover_mode {
                            self.recovered_errors.push(err);
                            self.bump();
                            self.recover_to_top_item();
                            continue;
                        } else {
                            return Err(err);
                        }
                    }
                    match self.parse_exports_decl() {
                        Ok(mut e) => {
                            e.trivia.leading = leading;
                            e.trivia.trailing = self.take_trailing_trivia();
                            last_span = e.span;
                            exports.push(e);
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Type) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_type_decl() {
                        Ok(mut t) => {
                            t.documentation = doc;
                            if let Some(sp) = private {
                                t.private = Some(sp);
                                t.span = sp.merge(t.span);
                            }
                            t.trivia.leading = leading;
                            t.trivia.trailing = self.take_trailing_trivia();
                            last_span = t.span;
                            items.push(CommonsItem::Type(t));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Fn) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_fn_decl() {
                        Ok(mut f) => {
                            f.documentation = doc;
                            if let Some(sp) = private {
                                f.private = Some(sp);
                                f.span = sp.merge(f.span);
                            }
                            f.trivia.leading = leading;
                            f.trivia.trailing = self.take_trailing_trivia();
                            last_span = f.span;
                            items.push(CommonsItem::Fn(f));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Capability) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_capability_decl() {
                        Ok(mut c) => {
                            c.documentation = doc;
                            c.trivia.leading = leading;
                            c.trivia.trailing = self.take_trailing_trivia();
                            last_span = c.span;
                            items.push(CommonsItem::Capability(c));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Provides) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_provider_decl() {
                        Ok(mut p) => {
                            p.documentation = doc;
                            p.trivia.leading = leading;
                            p.trivia.trailing = self.take_trailing_trivia();
                            last_span = p.span;
                            items.push(CommonsItem::Provider(p));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Service) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_service_decl() {
                        Ok(mut s) => {
                            s.documentation = doc;
                            s.trivia.leading = leading;
                            s.trivia.trailing = self.take_trailing_trivia();
                            last_span = s.span;
                            items.push(CommonsItem::Service(s));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // State-migrations S3 (#2248): `removed agent Old`. `removed`
                // is contextual — an identifier at item position, followed by
                // `agent` — the first contextual keyword in a context body.
                Some(TokenKind::Ident)
                    if self.peek().is_some_and(|t| self.slice(t.span) == "removed")
                        && self.tokens.get(self.pos + 1).map(|t| t.kind)
                            == Some(TokenKind::Agent) =>
                {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    let keyword = self.expect_ident("to start `removed agent`")?.span;
                    self.expect(TokenKind::Agent, "after `removed`")?;
                    let name = self.expect_ident("as the removed agent class")?;
                    let span = keyword.merge(name.span);
                    last_span = span;
                    removed_agents.push(RemovedAgentDecl {
                        keyword,
                        name,
                        documentation: doc,
                        span,
                        trivia: Trivia {
                            leading,
                            trailing: self.take_trailing_trivia(),
                        },
                    });
                    seen_item = true;
                }
                Some(TokenKind::Agent) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_agent_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Agent(a));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Actor) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_actor_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Actor(a));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // v0.223 (message-bundles slice 1): `messages` parses
                // syntactically in a context too — same shape as `Service`/
                // `Agent` inside an `adapter` body — so the checker can
                // reject it precisely (`bynk.messages.outside_commons`)
                // rather than the parser rejecting it per unit kind.
                Some(TokenKind::Messages) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_messages_decl() {
                        Ok(mut m) => {
                            m.documentation = doc;
                            if let Some(sp) = private {
                                m.private = Some(sp);
                                m.span = sp.merge(m.span);
                            }
                            m.trivia.leading = leading;
                            m.trivia.trailing = self.take_trailing_trivia();
                            last_span = m.span;
                            items.push(CommonsItem::Messages(m));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Event) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_event_decl() {
                        Ok(mut e) => {
                            e.documentation = doc;
                            e.trivia.leading = leading;
                            e.trivia.trailing = self.take_trailing_trivia();
                            last_span = e.span;
                            items.push(CommonsItem::Event(e));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #1883, slice 1a: `asset` parses wherever any item does;
                // placement (context or commons, never an adapter) is the
                // checker's call (`bynk.asset.outside_context_or_commons`),
                // the same split as `event`/`messages` above.
                Some(TokenKind::Asset) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_asset_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            if let Some(sp) = private {
                                a.private = Some(sp);
                                a.span = sp.merge(a.span);
                            }
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Asset(a));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #2387, slice 1: `embed` parses wherever any item does;
                // placement (context or commons, never an adapter) is the
                // checker's call (`bynk.embed.outside_context_or_commons`),
                // the same split as `event`/`messages` above.
                Some(TokenKind::Embed) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_embed_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            if let Some(sp) = private {
                                a.private = Some(sp);
                                a.span = sp.merge(a.span);
                            }
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Embed(a));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #2617: `const` parses wherever `embed` does; placement
                // (context or commons, never an adapter) is the checker's call
                // (`bynk.resolve.const_outside_context_or_commons`).
                Some(TokenKind::Const) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_const_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Const(a));
                            seen_item = true;
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                _ => {
                    let t = match self.peek() {
                        Some(t) => t,
                        None => {
                            return Err(CompileError::new(
                                "bynk.parse.unexpected_eof",
                                self.eof_span(),
                                "expected `}` to close the context body, found end of file",
                            ));
                        }
                    };
                    let err = CompileError::new(
                        "bynk.parse.expected_item",
                        t.span,
                        format!(
                            "expected a `type`, `fn`, `messages`, `uses`, `consumes`, `exports`, `capability`, `provides`, `service`, `agent`, or `actor` declaration, found {}",
                            t.kind.describe()
                        ),
                    );
                    if self.recover_mode {
                        self.recovered_errors.push(err);
                        self.bump();
                        self.recover_to_top_item();
                    } else {
                        return Err(err);
                    }
                }
            }
        }
        let mut trailing_comments = trailing_comments;
        let span = if brace {
            let end = self.expect(TokenKind::RBrace, "to close the context body")?;
            self.exit_item_loop();
            // See `parse_commons_body`'s matching call.
            trailing_comments.extend(self.take_epilogue_trivia());
            start.merge(end.span)
        } else {
            self.exit_item_loop();
            start.merge(last_span)
        };
        Ok(Context {
            name,
            items,
            uses,
            consumes,
            exports,
            documentation,
            form: if brace {
                CommonsForm::Brace
            } else {
                CommonsForm::Fragment
            },
            span,
            trivia: Trivia::default(),
            trailing_comments,
            removed_agents,
        })
    }

    /// Parse an `adapter` body in either brace (`brace = true`) or fragment
    /// (`brace = false`) form (v0.17 §3.1). An adapter accepts a `binding`
    /// clause plus the same item set as a context; service, agent and
    /// bodied-provider placement is validated by the checker, not rejected
    /// here, so the diagnostics can be precise. v0.18 admits `consumes`
    /// (braced form, adapter targets — also checked semantically).
    pub(super) fn parse_adapter_body(
        &mut self,
        start: Span,
        name: QualifiedName,
        documentation: Option<Doc>,
        brace: bool,
    ) -> Result<AdapterDecl, CompileError> {
        if brace {
            self.expect(TokenKind::LBrace, "after the adapter name")?;
        }
        self.enter_item_loop();
        let mut items = Vec::new();
        let mut uses = Vec::new();
        let mut exports = Vec::new();
        let mut consumes = Vec::new();
        let mut binding: Option<BindingDecl> = None;
        // Cover the header (`adapter <name>`) so the unit span stays valid even
        // when every item is dropped by error recovery (see commons fragment).
        let mut last_span = start.merge(name.span);
        let trailing_comments: Vec<Comment>;
        loop {
            let (mut leading, item_doc) = self.collect_item_lead();
            self.item_start = Some(self.pos);
            let private = self.take_private_modifier(false)?;
            match self.peek_kind() {
                Some(TokenKind::RBrace) if brace => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following declaration to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    trailing_comments = std::mem::take(&mut leading);
                    break;
                }
                None if !brace => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following declaration to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    leading.extend(self.take_epilogue_trivia());
                    trailing_comments = leading;
                    break;
                }
                Some(TokenKind::Binding) => {
                    self.keep_unattachable_doc(item_doc, &mut leading, "binding");
                    let mut b = self.parse_binding_decl()?;
                    b.trivia.leading = leading;
                    b.trivia.trailing = self.take_trailing_trivia();
                    last_span = b.span;
                    if binding.is_some() {
                        let err = CompileError::new(
                            "bynk.adapter.duplicate_binding",
                            b.span,
                            "an adapter may declare at most one `binding` clause",
                        );
                        self.handle_item_err(err)?;
                    } else {
                        binding = Some(b);
                    }
                }
                Some(TokenKind::Uses) => {
                    self.keep_unattachable_doc(item_doc, &mut leading, "uses");
                    match self.parse_uses_decl() {
                        Ok(mut u) => {
                            u.trivia.leading = leading;
                            u.trivia.trailing = self.take_trailing_trivia();
                            last_span = u.span;
                            uses.push(u);
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // v0.18: adapter-to-adapter capability dependencies. The braced-form
                // and adapter-target restrictions are checked semantically so the
                // diagnostics can be precise.
                Some(TokenKind::Consumes) => {
                    self.keep_unattachable_doc(item_doc, &mut leading, "consumes");
                    match self.parse_consumes_decl() {
                        Ok(mut c) => {
                            c.trivia.leading = leading;
                            c.trivia.trailing = self.take_trailing_trivia();
                            last_span = c.span;
                            consumes.push(c);
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Exports) => {
                    self.keep_unattachable_doc(item_doc, &mut leading, "exports");
                    match self.parse_exports_decl() {
                        Ok(mut e) => {
                            e.trivia.leading = leading;
                            e.trivia.trailing = self.take_trailing_trivia();
                            last_span = e.span;
                            exports.push(e);
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Type) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_type_decl() {
                        Ok(mut t) => {
                            t.documentation = doc;
                            if let Some(sp) = private {
                                t.private = Some(sp);
                                t.span = sp.merge(t.span);
                            }
                            t.trivia.leading = leading;
                            t.trivia.trailing = self.take_trailing_trivia();
                            last_span = t.span;
                            items.push(CommonsItem::Type(t));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Fn) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_fn_decl() {
                        Ok(mut f) => {
                            f.documentation = doc;
                            if let Some(sp) = private {
                                f.private = Some(sp);
                                f.span = sp.merge(f.span);
                            }
                            f.trivia.leading = leading;
                            f.trivia.trailing = self.take_trailing_trivia();
                            last_span = f.span;
                            items.push(CommonsItem::Fn(f));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Capability) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_capability_decl() {
                        Ok(mut c) => {
                            c.documentation = doc;
                            c.trivia.leading = leading;
                            c.trivia.trailing = self.take_trailing_trivia();
                            last_span = c.span;
                            items.push(CommonsItem::Capability(c));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Provides) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_provider_decl() {
                        Ok(mut p) => {
                            p.documentation = doc;
                            p.trivia.leading = leading;
                            p.trivia.trailing = self.take_trailing_trivia();
                            last_span = p.span;
                            items.push(CommonsItem::Provider(p));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // `service` and `agent` parse into items so the checker can
                // reject them precisely (`bynk.adapter.disallowed_item`).
                Some(TokenKind::Service) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_service_decl() {
                        Ok(mut s) => {
                            s.documentation = doc;
                            s.trivia.leading = leading;
                            s.trivia.trailing = self.take_trailing_trivia();
                            last_span = s.span;
                            items.push(CommonsItem::Service(s));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Agent) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_agent_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Agent(a));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Actor) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_actor_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Actor(a));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // `messages` parses into items too, so the checker can reject
                // it precisely (`bynk.messages.outside_commons`) — same
                // reasoning as `service`/`agent` above.
                Some(TokenKind::Messages) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_messages_decl() {
                        Ok(mut m) => {
                            m.documentation = doc;
                            if let Some(sp) = private {
                                m.private = Some(sp);
                                m.span = sp.merge(m.span);
                            }
                            m.trivia.leading = leading;
                            m.trivia.trailing = self.take_trailing_trivia();
                            last_span = m.span;
                            items.push(CommonsItem::Messages(m));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                Some(TokenKind::Event) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_event_decl() {
                        Ok(mut e) => {
                            e.documentation = doc;
                            e.trivia.leading = leading;
                            e.trivia.trailing = self.take_trailing_trivia();
                            last_span = e.span;
                            items.push(CommonsItem::Event(e));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #1883, slice 1a: `asset` parses wherever any item does;
                // placement (context or commons, never an adapter) is the
                // checker's call (`bynk.asset.outside_context_or_commons`),
                // the same split as `event`/`messages` above.
                Some(TokenKind::Asset) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_asset_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            if let Some(sp) = private {
                                a.private = Some(sp);
                                a.span = sp.merge(a.span);
                            }
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Asset(a));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #2387, slice 1: `embed` parses wherever any item does;
                // placement (context or commons, never an adapter) is the
                // checker's call (`bynk.embed.outside_context_or_commons`),
                // the same split as `event`/`messages` above.
                Some(TokenKind::Embed) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_embed_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            if let Some(sp) = private {
                                a.private = Some(sp);
                                a.span = sp.merge(a.span);
                            }
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Embed(a));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                // #2617: `const` parses wherever `embed` does; placement
                // (context or commons, never an adapter) is the checker's call
                // (`bynk.resolve.const_outside_context_or_commons`).
                Some(TokenKind::Const) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    match self.parse_const_decl() {
                        Ok(mut a) => {
                            a.documentation = doc;
                            a.trivia.leading = leading;
                            a.trivia.trailing = self.take_trailing_trivia();
                            last_span = a.span;
                            items.push(CommonsItem::Const(a));
                        }
                        Err(e) => self.handle_item_err(e)?,
                    }
                }
                _ => {
                    let t = match self.peek() {
                        Some(t) => t,
                        None => {
                            return Err(CompileError::new(
                                "bynk.parse.unexpected_eof",
                                self.eof_span(),
                                "expected `}` to close the adapter body, found end of file",
                            ));
                        }
                    };
                    let err = CompileError::new(
                        "bynk.parse.expected_item",
                        t.span,
                        format!(
                            "expected a `binding`, `type`, `fn`, `messages`, `uses`, `consumes`, `exports`, `capability`, or `provides` declaration, found {}",
                            t.kind.describe()
                        ),
                    );
                    if self.recover_mode {
                        self.recovered_errors.push(err);
                        self.bump();
                        self.recover_to_top_item();
                    } else {
                        return Err(err);
                    }
                }
            }
        }
        let mut trailing_comments = trailing_comments;
        let span = if brace {
            let end = self.expect(TokenKind::RBrace, "to close the adapter body")?;
            self.exit_item_loop();
            // Finding #30: unlike `parse_commons_brace`/`parse_context_brace`/
            // `parse_test_brace`, this brace-closing path never drained a
            // file-trailing comment — the fragment path just above already
            // does (`None if !brace`'s own `take_epilogue` call), so only an
            // `adapter … { … }` (not `adapter …\n\n…`) lost its last comment.
            trailing_comments.extend(self.take_epilogue_trivia());
            start.merge(end.span)
        } else {
            self.exit_item_loop();
            start.merge(last_span)
        };
        Ok(AdapterDecl {
            name,
            items,
            uses,
            exports,
            consumes,
            binding,
            documentation,
            form: if brace {
                CommonsForm::Brace
            } else {
                CommonsForm::Fragment
            },
            span,
            trivia: Trivia::default(),
            trailing_comments,
        })
    }

    /// Parse a `binding "<module>" requires { "pkg": "range", … }` clause
    /// (v0.17 §3.5). The `includes [ … ]` list (#2321) and the
    /// `requires { … }` map are optional.
    fn parse_binding_decl(&mut self) -> Result<BindingDecl, CompileError> {
        let kw = self.expect(TokenKind::Binding, "to start a `binding` declaration")?;
        let mod_tok = self.expect(
            TokenKind::StrLit,
            "the binding module path as a string literal",
        )?;
        let module = parse_string_literal(self.slice(mod_tok.span), mod_tok.span)?;
        let mut span = kw.span.merge(mod_tok.span);
        // #2321: `includes` is contextual (not a keyword), so `includes` stays a
        // usable name everywhere else.
        let mut includes = Vec::new();
        if self.peek_kind() == Some(TokenKind::Ident)
            && self
                .peek()
                .is_some_and(|t| self.slice(t.span) == "includes")
        {
            self.bump(); // `includes`
            self.expect(TokenKind::LBracket, "to open the `includes` list")?;
            loop {
                match self.peek_kind() {
                    Some(TokenKind::RBracket) => break,
                    Some(TokenKind::StrLit) => {
                        let tok = self.bump().unwrap();
                        includes.push(IncludeEntry {
                            path: parse_string_literal(self.slice(tok.span), tok.span)?,
                            span: tok.span,
                        });
                        self.eat(TokenKind::Comma);
                    }
                    _ => {
                        let (span, found) = match self.peek() {
                            Some(t) => (t.span, t.kind.describe()),
                            None => (self.eof_span(), "end of input"),
                        };
                        return Err(CompileError::new(
                            "bynk.parse.expected_item",
                            span,
                            format!(
                                "expected a string-literal path or `]` in the `includes` list, found {found}"
                            ),
                        ));
                    }
                }
            }
            let close = self.expect(TokenKind::RBracket, "to close the `includes` list")?;
            span = span.merge(close.span);
        }
        let mut requires = Vec::new();
        // v0.115: `requires` is a keyword (contract clauses). In a `binding`
        // it heads the optional dependency map (`requires { "pkg": "range" }`);
        // the two uses never overlap syntactically.
        if self.peek_kind() == Some(TokenKind::Requires) {
            self.bump(); // `requires`
            self.expect(TokenKind::LBrace, "to open the `requires` map")?;
            loop {
                match self.peek_kind() {
                    Some(TokenKind::RBrace) => break,
                    Some(TokenKind::StrLit) => {
                        let pkg_tok = self.bump().unwrap();
                        let package = parse_string_literal(self.slice(pkg_tok.span), pkg_tok.span)?;
                        self.expect(TokenKind::Colon, "after the package name")?;
                        let range_tok = self
                            .expect(TokenKind::StrLit, "the version range as a string literal")?;
                        let range =
                            parse_string_literal(self.slice(range_tok.span), range_tok.span)?;
                        requires.push(RequiresDep {
                            package,
                            range,
                            span: pkg_tok.span.merge(range_tok.span),
                        });
                        // optional trailing comma between entries
                        self.eat(TokenKind::Comma);
                    }
                    _ => {
                        // An unclosed `requires { …` map at end of input reaches
                        // here with no token — report at the EOF span rather than
                        // panicking on `peek().unwrap()` (a fuzz invariant: the
                        // parser never panics on any input).
                        let (span, found) = match self.peek() {
                            Some(t) => (t.span, t.kind.describe()),
                            None => (self.eof_span(), "end of input"),
                        };
                        return Err(CompileError::new(
                            "bynk.parse.expected_item",
                            span,
                            format!(
                                "expected a `\"package\": \"range\"` entry or `}}` in the `requires` map, found {found}"
                            ),
                        ));
                    }
                }
            }
            let close = self.expect(TokenKind::RBrace, "to close the `requires` map")?;
            span = span.merge(close.span);
        }
        Ok(BindingDecl {
            module,
            module_span: mod_tok.span,
            includes,
            requires,
            span,
            trivia: Trivia::default(),
        })
    }

    fn parse_qualified_name(&mut self) -> Result<QualifiedName, CompileError> {
        let first = self.expect_ident("for the commons name")?;
        let mut parts = vec![first];
        let mut span = parts[0].span;
        while self.eat(TokenKind::Dot).is_some() {
            let part = self.expect_ident("after `.` in the commons name")?;
            span = span.merge(part.span);
            parts.push(part);
        }
        Ok(QualifiedName { parts, span })
    }
}

impl<'a> Parser<'a> {
    // -- v0.5 declarations --

    fn parse_capability_decl(&mut self) -> Result<CapabilityDecl, CompileError> {
        let kw = self.expect(TokenKind::Capability, "to start a capability declaration")?;
        let name = self.expect_ident("after `capability`")?;
        // S4a (#1966): `capability Sessions = Kv @option(…)*`, a derived
        // capability, rather than a braced list of operations.
        if self.eat(TokenKind::Eq).is_some() {
            let base = self.expect_ident("as the base capability after `=`")?;
            let mut end = base.span;
            let mut annotations = Vec::new();
            while self.peek_kind() == Some(TokenKind::At) {
                let ann = self.parse_annotation()?;
                end = ann.span;
                annotations.push(ann);
            }
            return Ok(CapabilityDecl {
                trailing_comments: Vec::new(),
                name,
                ops: Vec::new(),
                derived: Some(DerivedCapability { base, annotations }),
                documentation: None,
                span: kw.span.merge(end),
                trivia: Trivia::default(),
            });
        }
        self.expect(TokenKind::LBrace, "to open the capability body")?;
        let mut ops = Vec::new();
        // #1756: comments (and an orphaned doc block) before the closing `}`.
        let trailing_comments: Vec<Comment>;
        loop {
            let (mut leading, item_doc) = self.collect_body_item_lead();
            match self.peek_kind() {
                Some(TokenKind::RBrace) => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following operation to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    trailing_comments = leading;
                    break;
                }
                Some(TokenKind::Fn) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    let mut op = self.parse_capability_op()?;
                    op.documentation = doc;
                    op.trivia.leading = leading;
                    op.trivia.trailing = self.take_trailing_trivia();
                    ops.push(op);
                }
                Some(_) => {
                    let t = self.peek().unwrap();
                    return Err(CompileError::new(
                        "bynk.parse.expected_capability_op",
                        t.span,
                        format!(
                            "expected `fn` to declare a capability operation, found {}",
                            t.kind.describe()
                        ),
                    ));
                }
                None => {
                    return Err(CompileError::new(
                        "bynk.parse.unexpected_eof",
                        self.eof_span(),
                        "expected `}` to close the capability body, found end of file",
                    ));
                }
            }
        }
        let close = self.expect(TokenKind::RBrace, "to close the capability body")?;
        if ops.is_empty() {
            return Err(CompileError::new(
                "bynk.parse.empty_capability",
                kw.span.merge(close.span),
                "a capability must declare at least one operation",
            ));
        }
        Ok(CapabilityDecl {
            trailing_comments,
            name,
            ops,
            derived: None,
            documentation: None,
            span: kw.span.merge(close.span),
            trivia: Trivia::default(),
        })
    }

    fn parse_capability_op(&mut self) -> Result<CapabilityOp, CompileError> {
        let kw = self.expect(TokenKind::Fn, "to start a capability operation")?;
        let name = self.expect_ident("as the capability operation name")?;
        // #926: `[T, …]` type parameters on the op itself, spelled identically
        // to `fn`/`type`'s own (`parse_optional_type_params`). Resolved only
        // from an explicit type argument at the call site — never inferred.
        let type_params = self.parse_optional_type_params()?;
        self.expect(TokenKind::LParen, "after the operation name")?;
        let mut params = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            params.push(self.parse_param()?);
            while self.eat(TokenKind::Comma).is_some() {
                params.push(self.parse_param()?);
            }
        }
        self.expect(TokenKind::RParen, "to close the operation parameter list")?;
        self.expect(TokenKind::Arrow, "before the operation return type")?;
        let return_type = self.parse_type_ref("as the operation return type")?;
        let end_span = return_type.span();
        Ok(CapabilityOp {
            name,
            type_params,
            params,
            return_type,
            documentation: None,
            span: kw.span.merge(end_span),
            trivia: Trivia::default(),
        })
    }

    /// Parse one capability reference in a `given` clause (v0.15 §3.2). A bare
    /// name (`Cap`) is a local capability; a dotted name (`B.Cap` /
    /// `platform.time.Clock`) refers to a capability provided by a consumed
    /// context — every segment but the last forms the context prefix.
    fn parse_cap_ref(&mut self) -> Result<CapRef, CompileError> {
        let role = "as a capability name in the `given` clause";
        let mut parts = vec![self.expect_ident(role)?];
        while self.peek_kind() == Some(TokenKind::Dot) {
            self.bump();
            parts.push(self.expect_ident(role)?);
        }
        let name = parts.pop().unwrap();
        let context = if parts.is_empty() {
            None
        } else {
            let qspan = parts
                .first()
                .unwrap()
                .span
                .merge(parts.last().unwrap().span);
            Some(QualifiedName { parts, span: qspan })
        };
        let span = context
            .as_ref()
            .map(|q| q.span.merge(name.span))
            .unwrap_or(name.span);
        Ok(CapRef {
            context,
            name,
            span,
        })
    }

    fn parse_provider_decl(&mut self) -> Result<ProviderDecl, CompileError> {
        let kw = self.expect(TokenKind::Provides, "to start a provider declaration")?;
        let capability = self.expect_ident("after `provides`")?;
        self.expect(TokenKind::Eq, "after the capability name")?;
        let provider_name = self.expect_ident("as the provider name")?;
        // v0.12: optional `given C1, C2` — capabilities the provider depends on.
        // v0.15: a dependency may be a cross-context capability (`given B.Cap`).
        let mut given = Vec::new();
        if self.peek_kind() == Some(TokenKind::Given) {
            self.bump();
            given.push(self.parse_cap_ref()?);
            while self.eat(TokenKind::Comma).is_some() {
                given.push(self.parse_cap_ref()?);
            }
        }
        // v0.17: a provider with **no** brace block is an *external* provider —
        // its implementation is supplied by an adapter's binding. The absence of
        // the brace block (not an empty one) is the signal. Whether this form is
        // legal here (adapter) or not (context) is decided by the checker, so the
        // parser accepts both shapes structurally.
        if self.peek_kind() != Some(TokenKind::LBrace) {
            let end = given.last().map(|g| g.span).unwrap_or(provider_name.span);
            return Ok(ProviderDecl {
                capability,
                provider_name,
                given,
                ops: Vec::new(),
                external: true,
                documentation: None,
                span: kw.span.merge(end),
                trivia: Trivia::default(),
            });
        }
        self.expect(TokenKind::LBrace, "to open the provider body")?;
        let mut ops = Vec::new();
        loop {
            let leading = self.take_leading_trivia();
            match self.peek_kind() {
                Some(TokenKind::RBrace) => break,
                Some(TokenKind::Fn) => {
                    let mut op = self.parse_provider_op()?;
                    op.trivia.leading = leading;
                    op.trivia.trailing = self.take_trailing_trivia();
                    ops.push(op);
                }
                Some(_) => {
                    let t = self.peek().unwrap();
                    return Err(CompileError::new(
                        "bynk.parse.expected_provider_op",
                        t.span,
                        format!(
                            "expected `fn` to declare a provider operation, found {}",
                            t.kind.describe()
                        ),
                    ));
                }
                None => {
                    return Err(CompileError::new(
                        "bynk.parse.unexpected_eof",
                        self.eof_span(),
                        "expected `}` to close the provider body, found end of file",
                    ));
                }
            }
        }
        let close = self.expect(TokenKind::RBrace, "to close the provider body")?;
        Ok(ProviderDecl {
            capability,
            provider_name,
            given,
            ops,
            external: false,
            documentation: None,
            span: kw.span.merge(close.span),
            trivia: Trivia::default(),
        })
    }

    fn parse_provider_op(&mut self) -> Result<ProviderOp, CompileError> {
        let kw = self.expect(TokenKind::Fn, "to start a provider operation")?;
        let name = self.expect_ident("as the provider operation name")?;
        self.expect(TokenKind::LParen, "after the operation name")?;
        let mut params = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            params.push(self.parse_param()?);
            while self.eat(TokenKind::Comma).is_some() {
                params.push(self.parse_param()?);
            }
        }
        self.expect(TokenKind::RParen, "to close the operation parameter list")?;
        self.expect(TokenKind::Arrow, "before the operation return type")?;
        let return_type = self.parse_type_ref("as the operation return type")?;
        let body = self.parse_block("to open the provider operation body")?;
        let span = kw.span.merge(body.span);
        Ok(ProviderOp {
            name,
            params,
            return_type,
            body,
            span,
            trivia: Trivia::default(),
        })
    }

    /// v0.45: an actor declaration — a nominal boundary contract.
    ///
    /// Normal form: `actor Name { auth = Scheme }` or
    /// `actor Name { auth = Scheme, identity = Type }`.
    ///
    /// Refinement form: `actor Admin = User where <predicate>` — narrows a base
    /// actor by an authorisation claim (ADR 0091). The predicate is parsed as a
    /// full expression and restricted by a static-semantics rule to the closed
    /// actor-claim catalogue.
    fn parse_actor_decl(&mut self) -> Result<ActorDecl, CompileError> {
        let kw = self.expect(TokenKind::Actor, "to start an actor declaration")?;
        let name = self.expect_ident("after `actor`")?;

        // Refinement form: `actor Name = Base where <predicate>`. The predicate
        // is parsed as a full expression (`parse_expr` below); a static-semantics
        // rule restricts it to the closed actor-claim catalogue
        // (`bynk.actor.refinement_predicate_unsupported` / `…_base_unsupported`).
        if self.peek_kind() == Some(TokenKind::Eq) {
            self.bump();
            let base = self.expect_ident("as the base actor after `=`")?;
            self.expect(TokenKind::Where, "before the actor refinement predicate")?;
            let predicate = self.parse_expr()?;
            let span = kw.span.merge(predicate.span);
            return Ok(ActorDecl {
                name,
                auth: None,
                auth_config: Vec::new(),
                identity: None,
                refinement: Some(ActorRefinement {
                    base,
                    predicate,
                    span,
                }),
                documentation: None,
                span,
                trivia: Trivia::default(),
                auth_trivia: Trivia::default(),
                identity_trivia: Trivia::default(),
                trailing_comments: Vec::new(),
            });
        }

        // Normal form: `actor Name { auth = Scheme (, identity = Type)? }`.
        self.expect(TokenKind::LBrace, "to open the actor body")?;
        let auth_leading = self.take_leading_trivia();
        let auth_kw = self.expect_ident("(`auth`) to start the actor body")?;
        if auth_kw.name != "auth" {
            return Err(CompileError::new(
                "bynk.parse.expected_token",
                auth_kw.span,
                format!(
                    "expected `auth` in the actor body, found `{}`",
                    auth_kw.name
                ),
            )
            .with_note("an actor body begins with `auth = <Scheme>`"));
        }
        self.expect(TokenKind::Eq, "after `auth`")?;
        // The scheme name is an identifier, except `None` (which is also the
        // `Option` keyword) — accept that token here as the scheme name.
        let auth = if self.peek_kind() == Some(TokenKind::None) {
            let t = self.expect(TokenKind::None, "as the authentication scheme")?;
            Ident {
                name: "None".to_string(),
                span: t.span,
            }
        } else {
            self.expect_ident("as the authentication scheme after `auth =`")?
        };

        // v0.47/v0.51: a scheme may carry a keyed config —
        // `Scheme(key = value, …)` (e.g. `Bearer(secret = "…")`,
        // `Signature(secret = "…", header = "…", timestamp = "…", tolerance =
        // 300)`). Values are string or integer literals; the checker validates
        // which keys each scheme requires/allows.
        let mut auth_config = Vec::new();
        if self.peek_kind() == Some(TokenKind::LParen) {
            self.bump();
            loop {
                let key = self.expect_ident("as a scheme config key")?;
                self.expect(TokenKind::Eq, "after the scheme config key")?;
                let (value, vspan) = match self.peek_kind() {
                    Some(TokenKind::StrLit) => {
                        let t = self.expect(TokenKind::StrLit, "as a scheme config value")?;
                        (
                            SchemeArgValue::Str(parse_string_literal(self.slice(t.span), t.span)?),
                            t.span,
                        )
                    }
                    Some(TokenKind::IntLit) => {
                        let t = self.expect(TokenKind::IntLit, "as a scheme config value")?;
                        // v0.142 (ADR 0166): parse from the separator-free form.
                        let n: i64 = crate::lexer::strip_digit_separators(self.slice(t.span))
                            .parse()
                            .map_err(|_| {
                                CompileError::new(
                                    "bynk.parse.expected_token",
                                    t.span,
                                    "invalid integer in scheme config".to_string(),
                                )
                            })?;
                        (SchemeArgValue::Int(n), t.span)
                    }
                    _ => {
                        let t = self.peek();
                        return Err(CompileError::new(
                            "bynk.parse.expected_token",
                            t.map(|t| t.span).unwrap_or_else(|| self.eof_span()),
                            "expected a string or integer scheme config value".to_string(),
                        ));
                    }
                };
                auth_config.push(SchemeArg {
                    key,
                    value,
                    span: vspan,
                });
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
                if self.peek_kind() == Some(TokenKind::RParen) {
                    break; // trailing comma
                }
            }
            self.expect(TokenKind::RParen, "to close the scheme config")?;
        }

        // #1797: `auth`'s end-of-line comment sits after its `,`, if any.
        let comma = self.eat(TokenKind::Comma);
        let auth_trivia = Trivia {
            leading: auth_leading,
            trailing: self.take_trailing_trivia(),
        };
        let mut identity = None;
        let mut identity_trivia = Trivia::default();
        if comma.is_some() {
            identity_trivia.leading = self.take_leading_trivia();
            let id_kw = self.expect_ident("(`identity`) after `,`")?;
            if id_kw.name != "identity" {
                return Err(CompileError::new(
                    "bynk.parse.expected_token",
                    id_kw.span,
                    format!("expected `identity`, found `{}`", id_kw.name),
                )
                .with_note("the only actor field after `auth` is `identity = <Type>`"));
            }
            self.expect(TokenKind::Eq, "after `identity`")?;
            identity = Some(self.parse_type_ref("as the actor identity type")?);
            identity_trivia.trailing = self.take_trailing_trivia();
        }

        let trailing_comments = self.take_leading_trivia();
        let close = self.expect(TokenKind::RBrace, "to close the actor body")?;
        let span = kw.span.merge(close.span);
        Ok(ActorDecl {
            name,
            auth: Some(auth),
            auth_config,
            identity,
            refinement: None,
            documentation: None,
            span,
            trivia: Trivia::default(),
            auth_trivia,
            identity_trivia,
            trailing_comments,
        })
    }

    fn parse_service_decl(&mut self) -> Result<ServiceDecl, CompileError> {
        let kw = self.expect(TokenKind::Service, "to start a service declaration")?;
        let name = self.expect_ident("after `service`")?;
        let protocol = self.parse_service_protocol()?;
        // v0.155: optional service-level `by`/`given` defaults on the header, `by`
        // first — the ambient contract every handler inherits unless it declares its
        // own. Reuses the shared handler clause parsers; the checker validates the
        // default and the normalization pass injects it into omitting handlers.
        let default_by = self.parse_by_clause()?;
        let default_given = self.parse_given_clause()?;
        self.expect(TokenKind::LBrace, "to open the service body")?;
        let mut handlers = Vec::new();
        let mut cors: Option<CorsPolicy> = None;
        let mut security: Option<SecurityPolicy> = None;
        let mut limits: Option<LimitsPolicy> = None;
        // #1756: comments (and an orphaned doc block) before the closing `}`.
        let trailing_comments: Vec<Comment>;
        loop {
            let (mut leading, item_doc) = self.collect_item_lead();
            match self.peek_kind() {
                Some(TokenKind::RBrace) => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following handler to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    trailing_comments = leading;
                    break;
                }
                // `cors { … }` is a contextual keyword (like `store`/`key`): the
                // identifier `cors` in service-body item position introduces the
                // CORS policy, so it stays usable as an ordinary identifier
                // elsewhere. At most one per service.
                Some(TokenKind::Ident) if self.peek_is_cors_kw() => {
                    if let Some(doc) = item_doc {
                        self.warn_orphan_before_policy(doc.span);
                        keep_orphan(&mut leading, doc);
                    }
                    let policy = self.parse_cors_policy(leading)?;
                    if cors.is_some() {
                        return Err(CompileError::new(
                            "bynk.parse.duplicate_cors",
                            policy.span,
                            "a service declares at most one `cors { }` policy",
                        ));
                    }
                    self.check_policy_order(
                        "cors",
                        policy.span,
                        &[
                            ("security", security.is_some()),
                            ("limits", limits.is_some()),
                        ],
                        !handlers.is_empty(),
                    )?;
                    cors = Some(policy);
                }
                // `security { … }` is a contextual keyword like `cors` (v0.141,
                // ADR 0164): the identifier `security` in service-body item
                // position introduces the security-headers policy, so it stays
                // usable as an ordinary identifier elsewhere. At most one per
                // service.
                Some(TokenKind::Ident) if self.peek_is_security_kw() => {
                    if let Some(doc) = item_doc {
                        self.warn_orphan_before_policy(doc.span);
                        keep_orphan(&mut leading, doc);
                    }
                    let policy = self.parse_security_policy(leading)?;
                    if security.is_some() {
                        return Err(CompileError::new(
                            "bynk.parse.duplicate_security",
                            policy.span,
                            "a service declares at most one `security { }` policy",
                        ));
                    }
                    self.check_policy_order(
                        "security",
                        policy.span,
                        &[("limits", limits.is_some())],
                        !handlers.is_empty(),
                    )?;
                    security = Some(policy);
                }
                // `limits { … }` is a contextual keyword like `cors`/`security`
                // (v0.142, ADR 0165): the identifier `limits` in service-body item
                // position introduces the request-body-size policy, so it stays
                // usable as an ordinary identifier elsewhere. At most one per
                // service.
                Some(TokenKind::Ident) if self.peek_is_limits_kw() => {
                    if let Some(doc) = item_doc {
                        self.warn_orphan_before_policy(doc.span);
                        keep_orphan(&mut leading, doc);
                    }
                    let policy = self.parse_limits_policy(leading)?;
                    if limits.is_some() {
                        return Err(CompileError::new(
                            "bynk.parse.duplicate_limits",
                            policy.span,
                            "a service declares at most one `limits { }` policy",
                        ));
                    }
                    self.check_policy_order("limits", policy.span, &[], !handlers.is_empty())?;
                    limits = Some(policy);
                }
                // A leading `@name(args)` introduces a handler-position annotation
                // (v0.140): consume the annotation run, then the `on` handler it
                // decorates. Validation gates `@cache` to `on GET` and `@limit` to
                // a body-taking route downstream.
                Some(TokenKind::At) => {
                    let annotations = self.parse_handler_annotations()?;
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    let mut h = self.parse_handler(false, annotations)?;
                    h.documentation = doc;
                    h.trivia.leading = leading;
                    h.trivia.trailing = self.take_trailing_trivia();
                    handlers.push(h);
                }
                Some(TokenKind::On) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    let mut h = self.parse_handler(false, Vec::new())?;
                    h.documentation = doc;
                    h.trivia.leading = leading;
                    h.trivia.trailing = self.take_trailing_trivia();
                    handlers.push(h);
                }
                Some(_) => {
                    let t = self.peek().unwrap();
                    return Err(CompileError::new(
                        "bynk.parse.expected_handler",
                        t.span,
                        format!(
                            "expected `on` to start a handler, found {}",
                            t.kind.describe()
                        ),
                    ));
                }
                None => {
                    return Err(CompileError::new(
                        "bynk.parse.unexpected_eof",
                        self.eof_span(),
                        "expected `}` to close the service body, found end of file",
                    ));
                }
            }
        }
        let close = self.expect(TokenKind::RBrace, "to close the service body")?;
        if handlers.is_empty() {
            return Err(CompileError::new(
                "bynk.parse.empty_service",
                kw.span.merge(close.span),
                "a service must declare at least one handler",
            ));
        }
        Ok(ServiceDecl {
            trailing_comments,
            name,
            protocol,
            default_by,
            default_given,
            cors,
            security,
            limits,
            handlers,
            documentation: None,
            span: kw.span.merge(close.span),
            trivia: Trivia::default(),
        })
    }

    /// True when the next token is the contextual keyword `cors` (an identifier
    /// literally spelled `cors`), introducing the CORS policy section.
    fn peek_is_cors_kw(&self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Ident && self.slice(t.span) == "cors")
    }

    /// True when the next token is the contextual keyword `security` (an
    /// identifier literally spelled `security`), introducing the security-headers
    /// policy section.
    fn peek_is_security_kw(&self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Ident && self.slice(t.span) == "security")
    }

    /// True when the next token is the contextual keyword `limits` (an identifier
    /// literally spelled `limits`), introducing the request-body-size policy
    /// section.
    fn peek_is_limits_kw(&self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Ident && self.slice(t.span) == "limits")
    }

    /// #1756: a doc block before an adapter's `binding`/`uses`/`consumes`/
    /// `exports` clause documents nothing, so it is an orphan: warn, and keep it
    /// in `leading`. It used to be dropped with no warning at all.
    fn keep_unattachable_doc(
        &mut self,
        doc: Option<DocLead>,
        leading: &mut Vec<Comment>,
        clause: &str,
    ) {
        if let Some(doc) = doc {
            self.warnings.push(CompileError::new(
                "bynk.parse.orphan_doc_block",
                doc.span,
                format!(
                    "documentation block before `{clause}` is not attached; only declarations carry docs"
                ),
            ));
            keep_orphan(leading, doc);
        }
    }

    /// #1784: a service's policies come before its handlers, in the order
    /// `cors`, `security`, `limits`, each optional: the order `service_decl` has
    /// in the tree-sitter grammar and the spec (§4.4.1a), and the one `fmt`
    /// writes. This parser used to accept them anywhere in the body. `later`
    /// names the policies that must follow `keyword`, with whether each has
    /// already been seen.
    fn check_policy_order(
        &self,
        keyword: &str,
        span: Span,
        later: &[(&str, bool)],
        handlers_seen: bool,
    ) -> Result<(), CompileError> {
        if handlers_seen {
            return Err(CompileError::new(
                "bynk.parse.policy_order",
                span,
                format!("`{keyword} {{ }}` must come before the service's handlers"),
            )
            .with_note(
                "a service's policies open its body, before any handler, in the order `cors`, `security`, `limits`",
            ));
        }
        if let Some((after, _)) = later.iter().find(|(_, seen)| *seen) {
            return Err(CompileError::new(
                "bynk.parse.policy_order",
                span,
                format!("`{keyword} {{ }}` must come before `{after} {{ }}`"),
            )
            .with_note("a service's policies come in the order `cors`, `security`, `limits`"));
        }
        Ok(())
    }

    /// #1756: a doc block before a `cors`/`security`/`limits` policy documents
    /// nothing (a policy carries no doc), so it is an orphan.
    fn warn_orphan_before_policy(&mut self, doc_span: Span) {
        self.warnings.push(CompileError::new(
            "bynk.parse.orphan_doc_block",
            doc_span,
            "documentation block before a service policy is not attached; only handlers in a service body carry docs",
        ));
    }

    /// Parse a `cors { name: value, … }` policy (v0.131, ADR 0159). Fields are
    /// parsed leniently as `name: expr` pairs; the checker validates the field
    /// names (closed set) and the value shapes. A trailing comma is allowed and
    /// newlines separate fields, mirroring a record construction.
    fn parse_cors_policy(&mut self, leading: Vec<Comment>) -> Result<CorsPolicy, CompileError> {
        let kw = self.expect_ident("to start a `cors` policy")?;
        let body = self.parse_policy_body("cors")?;
        Ok(CorsPolicy {
            fields: body
                .fields
                .into_iter()
                .map(|f| CorsField {
                    name: f.name,
                    value: f.value,
                    span: f.span,
                    trivia: f.trivia,
                })
                .collect(),
            span: kw.span.merge(body.close),
            trailing_comments: body.trailing_comments,
            trivia: Trivia {
                leading,
                trailing: self.take_trailing_trivia(),
            },
        })
    }

    /// Parse a `security { name: value, … }` policy (v0.141, ADR 0164), as
    /// [`Self::parse_cors_policy`]; the checker validates the closed field set
    /// `hsts`/`nosniff` and the value shapes.
    fn parse_security_policy(
        &mut self,
        leading: Vec<Comment>,
    ) -> Result<SecurityPolicy, CompileError> {
        let kw = self.expect_ident("to start a `security` policy")?;
        let body = self.parse_policy_body("security")?;
        Ok(SecurityPolicy {
            fields: body
                .fields
                .into_iter()
                .map(|f| SecurityField {
                    name: f.name,
                    value: f.value,
                    span: f.span,
                    trivia: f.trivia,
                })
                .collect(),
            span: kw.span.merge(body.close),
            trailing_comments: body.trailing_comments,
            trivia: Trivia {
                leading,
                trailing: self.take_trailing_trivia(),
            },
        })
    }

    /// Parse a `limits { name: value, … }` policy (v0.142, ADR 0165), as
    /// [`Self::parse_cors_policy`]; the checker validates the closed field set
    /// `maxBody` and the value shape (a positive `Int`).
    fn parse_limits_policy(&mut self, leading: Vec<Comment>) -> Result<LimitsPolicy, CompileError> {
        let kw = self.expect_ident("to start a `limits` policy")?;
        let body = self.parse_policy_body("limits")?;
        Ok(LimitsPolicy {
            fields: body
                .fields
                .into_iter()
                .map(|f| LimitsField {
                    name: f.name,
                    value: f.value,
                    span: f.span,
                    trivia: f.trivia,
                })
                .collect(),
            span: kw.span.merge(body.close),
            trailing_comments: body.trailing_comments,
            trivia: Trivia {
                leading,
                trailing: self.take_trailing_trivia(),
            },
        })
    }

    /// The brace-delimited `name: value` fields of a `cors`/`security`/`limits`
    /// policy (`policy` names it in errors), from `{` through `}`. #1786: each
    /// field keeps the comments above it and its end-of-line comment, and the
    /// comments before `}` are kept too; they used to be discarded. A doc block
    /// documents nothing here, so it is an orphan.
    fn parse_policy_body(&mut self, policy: &str) -> Result<PolicyBody, CompileError> {
        self.expect(
            TokenKind::LBrace,
            &format!("to open the `{policy}` policy body"),
        )?;
        let mut fields = Vec::new();
        let trailing_comments = loop {
            let (mut leading, item_doc) = self.collect_body_item_lead();
            if let Some(doc) = item_doc {
                self.warnings.push(CompileError::new(
                    "bynk.parse.orphan_doc_block",
                    doc.span,
                    format!("documentation block in a `{policy}` policy is not attached; policy fields carry no docs"),
                ));
                keep_orphan(&mut leading, doc);
            }
            match self.peek_kind() {
                Some(TokenKind::RBrace) => break leading,
                Some(_) => {
                    let name = self.expect_ident(&format!("as a `{policy}` policy field name"))?;
                    self.expect(
                        TokenKind::Colon,
                        &format!("after the `{policy}` field name"),
                    )?;
                    let value = self.parse_expr()?;
                    let span = name.span.merge(value.span);
                    let _ = self.eat(TokenKind::Comma);
                    let trivia = Trivia {
                        leading,
                        trailing: self.take_trailing_trivia(),
                    };
                    fields.push(PolicyField {
                        name,
                        value,
                        span,
                        trivia,
                    });
                }
                None => {
                    return Err(CompileError::new(
                        "bynk.parse.unexpected_eof",
                        self.eof_span(),
                        format!("expected `}}` to close the `{policy}` policy, found end of file"),
                    ));
                }
            }
        };
        let close = self.expect(
            TokenKind::RBrace,
            &format!("to close the `{policy}` policy body"),
        )?;
        Ok(PolicyBody {
            fields,
            trailing_comments,
            close: close.span,
        })
    }

    /// Parse the optional `from <protocol>` header clause (v0.44). Absent ⇒
    /// `Call` (the contract-mediated internal-RPC default). `from queue("name")`
    /// carries its bound queue; `from http`/`from cron` carry no binding.
    fn parse_service_protocol(&mut self) -> Result<ServiceProtocol, CompileError> {
        if self.eat(TokenKind::From).is_none() {
            return Ok(ServiceProtocol::Call);
        }
        match self.peek_kind() {
            Some(TokenKind::Http) => {
                self.bump();
                Ok(ServiceProtocol::Http)
            }
            Some(TokenKind::Cron) => {
                self.bump();
                Ok(ServiceProtocol::Cron)
            }
            Some(TokenKind::Queue) => {
                self.bump();
                self.expect(
                    TokenKind::LParen,
                    "expected a queue binding `(\"name\")` after `from queue`",
                )?;
                let name_tok = self.expect(
                    TokenKind::StrLit,
                    "expected the bound queue name as a string literal",
                )?;
                let name = parse_string_literal(self.slice(name_tok.span), name_tok.span)?;
                self.expect(TokenKind::RParen, "to close the queue binding")?;
                Ok(ServiceProtocol::Queue { name })
            }
            // v0.103 (real-time track slice 3): `from websocket(in: T, out: T)`.
            // `websocket` is a contextual identifier (not a keyword), like the
            // built-in type names. #548: lowercased from `WebSocket` to match the
            // other protocol sources.
            Some(TokenKind::Ident)
                if self
                    .peek()
                    .is_some_and(|t| self.slice(t.span) == "websocket") =>
            {
                self.bump();
                self.expect(
                    TokenKind::LParen,
                    "expected `(in: ClientFrame, out: ServerFrame)` after `from websocket`",
                )?;
                let in_label = self.expect_ident("(`in:`) in the WebSocket header")?;
                if in_label.name != "in" {
                    return Err(CompileError::new(
                        "bynk.service.websocket_header",
                        in_label.span,
                        "the WebSocket header binds frame types as `in: <type>, out: <type>`",
                    ));
                }
                self.expect(TokenKind::Colon, "after `in`")?;
                let in_type = self.parse_type_ref("as the WebSocket `in:` frame type")?;
                self.expect(TokenKind::Comma, "between the `in:` and `out:` frame types")?;
                let out_label = self.expect_ident("(`out:`) in the WebSocket header")?;
                if out_label.name != "out" {
                    return Err(CompileError::new(
                        "bynk.service.websocket_header",
                        out_label.span,
                        "the WebSocket header binds frame types as `in: <type>, out: <type>`",
                    ));
                }
                self.expect(TokenKind::Colon, "after `out`")?;
                let out_type = self.parse_type_ref("as the WebSocket `out:` frame type")?;
                self.expect(TokenKind::RParen, "to close the WebSocket header")?;
                Ok(ServiceProtocol::WebSocket { in_type, out_type })
            }
            // Events track, slice 0 (spine #936): `from Events(E)`, optionally
            // filtered by a slice-1 structural pattern, `from Events(E {
            // field: value, .. })`. `Events`, capitalised, is matched as plain
            // `Ident` text the same way `websocket` is matched lowercase — it
            // names the `Events` capability directly (every first-party
            // capability is already an unreserved PascalCase identifier), so
            // this costs no lexer reservation.
            Some(TokenKind::Ident)
                if self.peek().is_some_and(|t| self.slice(t.span) == "Events") =>
            {
                self.bump();
                self.expect(
                    TokenKind::LParen,
                    "expected the subscribed event type `(EventType)` after `from Events`",
                )?;
                let event_type = self.parse_type_ref("as the `from Events(...)` event type")?;
                let pattern = if self.peek_kind() == Some(TokenKind::LBrace) {
                    Some(self.parse_event_pattern()?)
                } else {
                    None
                };
                self.expect(TokenKind::RParen, "to close the `from Events(...)` header")?;
                let schema_dispatch = self.parse_schema_dispatch_clause()?;
                Ok(ServiceProtocol::Events {
                    event_type,
                    pattern,
                    schema_dispatch,
                })
            }
            _ => {
                let (span, found) = match self.peek() {
                    Some(t) => (t.span, t.kind.describe()),
                    None => (self.eof_span(), "end of file"),
                };
                Err(CompileError::new(
                    "bynk.service.unknown_protocol",
                    span,
                    format!(
                        "unknown protocol after `from` — found {found}, expected `http`, `cron`, `queue`, `websocket`, or `Events`"
                    ),
                )
                .with_note(
                    "protocols are a closed set; Kafka and MQTT are transports, not protocols — \
                     use `from queue` and bind the broker at the platform layer",
                ))
            }
        }
    }

    /// Parse an events subscription pattern (Events track, slice 1, spine
    /// #936): `{ field: value, .. }`. The caller has already confirmed the
    /// leading token is `{`. A trailing `..` is required whenever any field
    /// is listed — `from Events(E { })` is rejected, steering toward the
    /// bare `from Events(E)` pattern-less form.
    fn parse_event_pattern(&mut self) -> Result<EventPattern, CompileError> {
        let open = self.expect(TokenKind::LBrace, "to start an events subscription pattern")?;
        let mut fields = Vec::new();
        loop {
            // A `..` ends the field list normally; a `}` here means no
            // fields were listed at all (`{ }`) — both fall through to the
            // empty-pattern check below rather than `expect_ident` failing
            // on `}` with a less specific message.
            if matches!(
                self.peek_kind(),
                Some(TokenKind::DotDot) | Some(TokenKind::RBrace)
            ) {
                break;
            }
            let name = self.expect_ident("as an events subscription pattern field name")?;
            self.expect(
                TokenKind::Colon,
                "expected `:` after an events subscription pattern field name",
            )?;
            let value = self.parse_event_pattern_value()?;
            let field_span = name.span.merge(value.span());
            fields.push(EventPatternField {
                name,
                value,
                span: field_span,
            });
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        if fields.is_empty() {
            let (span, found) = match self.peek() {
                Some(t) => (t.span, t.kind.describe()),
                None => (self.eof_span(), "end of file"),
            };
            return Err(CompileError::new(
                "bynk.parse.event_pattern_empty",
                span,
                format!(
                    "an events subscription pattern must list at least one field — found {found}"
                ),
            )
            .with_note("use `from Events(EventType)` (no braces) for an unfiltered subscription"));
        }
        let rest = self.expect(
            TokenKind::DotDot,
            "after an events subscription pattern's fields — every listed field leaves the rest unconstrained, and that must be written explicitly",
        )?;
        let close = self.expect(TokenKind::RBrace, "to close an events subscription pattern")?;
        Ok(EventPattern {
            fields,
            rest_span: rest.span,
            span: open.span.merge(close.span),
        })
    }

    /// Parse one events subscription pattern field's value: a literal
    /// (`31`, `"english"`, `true`/`false`, an optional leading `-` on an
    /// integer) or a nullary sum-variant reference, bare (`Domestic`) or
    /// qualified (`Region.Domestic`). No nested record sub-patterns in v1.
    fn parse_event_pattern_value(&mut self) -> Result<EventPatternValue, CompileError> {
        if let Some(t) = self.peek() {
            let is_literal_lead = matches!(
                t.kind,
                TokenKind::IntLit | TokenKind::StrLit | TokenKind::True | TokenKind::False
            ) || (t.kind == TokenKind::Minus
                && self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(TokenKind::IntLit));
            if is_literal_lead {
                let (value, span) = self.parse_literal_value()?;
                return Ok(EventPatternValue::Literal { value, span });
            }
        }
        let first = self.expect_ident("as an events subscription pattern field value")?;
        let (type_name, variant) = if self.eat(TokenKind::Dot).is_some() {
            let v = self.expect_ident("after `.` in a qualified events pattern variant")?;
            (Some(first), v)
        } else {
            (None, first)
        };
        let span = type_name
            .as_ref()
            .map(|t| t.span)
            .unwrap_or(variant.span)
            .merge(variant.span);
        Ok(EventPatternValue::Variant {
            type_name,
            variant,
            span,
        })
    }

    /// Parse an optional `via schema(N)` clause (Events track, slice 4,
    /// spine #936), following the `from Events(...)` header's closing `)`.
    /// `via`/`schema` are matched as plain `Ident` text, the same way
    /// `websocket`/`Events` are — no lexer reservation. `N` admits an
    /// optional leading `-` (mirroring `parse_literal_value`'s own
    /// int-literal handling) so a non-positive value is a *checker* error
    /// (`bynk.event.bad_schema_dispatch`), not a parse error — the same
    /// permissive-parse-then-checker-validate split `@schema(N)` uses.
    fn parse_schema_dispatch_clause(&mut self) -> Result<Option<SchemaDispatch>, CompileError> {
        let Some(via_tok) = self.peek() else {
            return Ok(None);
        };
        if via_tok.kind != TokenKind::Ident || self.slice(via_tok.span) != "via" {
            return Ok(None);
        }
        self.bump();
        let schema_kw = self.expect_ident("(`schema`) after `via`")?;
        if schema_kw.name != "schema" {
            return Err(CompileError::new(
                "bynk.service.unknown_via_clause",
                schema_kw.span,
                format!(
                    "unknown `via` clause `{}` — expected `schema`",
                    schema_kw.name
                ),
            )
            .with_note("`via` clauses are a closed set; only `via schema(...)` exists today"));
        }
        self.expect(TokenKind::LParen, "expected `(N)` after `via schema`")?;
        let neg = self.eat(TokenKind::Minus);
        let num_tok = self.expect(
            TokenKind::IntLit,
            "expected an `Int` literal as the `via schema(...)` version",
        )?;
        let slice = self.slice(num_tok.span);
        let mut version: i64 = crate::lexer::strip_digit_separators(slice)
            .parse()
            .map_err(|_| {
                CompileError::new(
                    "bynk.lex.integer_overflow",
                    num_tok.span,
                    format!("integer literal `{slice}` out of 64-bit range"),
                )
            })?;
        if neg.is_some() {
            version = -version;
        }
        let close = self.expect(TokenKind::RParen, "to close `via schema(...)`")?;
        let span = via_tok.span.merge(close.span);
        Ok(Some(SchemaDispatch {
            pattern: SchemaVersionPattern::Literal(version),
            span,
        }))
    }

    fn parse_agent_decl(&mut self) -> Result<AgentDecl, CompileError> {
        let kw = self.expect(TokenKind::Agent, "to start an agent declaration")?;
        let name = self.expect_ident("after `agent`")?;
        // State-migrations S3 (#2248): `agent New replaces Old` — `replaces`
        // is contextual, recognised only between the name and the body.
        let replaces = if matches!(self.peek(), Some(t) if t.kind == TokenKind::Ident && self.slice(t.span) == "replaces")
        {
            self.bump();
            Some(self.expect_ident("as the agent class `replaces` renames")?)
        } else {
            None
        };
        self.expect(TokenKind::LBrace, "to open the agent body")?;
        let key_leading = self.take_leading_trivia();
        // key id: Type
        // The `key` keyword is recognised as an identifier with the literal
        // name "key" — we don't have a dedicated keyword so it can be a
        // method name elsewhere. v0.5 reserves it only inside an agent body.
        let key_ident = self.expect_ident("(`key`) at the start of the agent body")?;
        if key_ident.name != "key" {
            return Err(CompileError::new(
                "bynk.parse.expected_agent_key",
                key_ident.span,
                format!(
                    "expected `key id: Type` at the start of the agent body, found `{}`",
                    key_ident.name
                ),
            ));
        }
        let key_name = self.expect_ident("as the agent key field name")?;
        self.expect(TokenKind::Colon, "after the agent key field name")?;
        let key_type = self.parse_type_ref("as the agent key type")?;
        let key_trivia = Trivia {
            leading: key_leading,
            trailing: self.take_trailing_trivia(),
        };
        // Agent body — a pinned four-phase parse (identity → storage → contracts
        // → behaviour). v0.81 (storage track): the storage phase is the legacy
        // `state { }` block and/or the successor `store` fields, which coexist
        // during the track (ADR 0108 D3). All body items are doc-prefixed, so a
        // single loop collects the lead once and dispatches; ordering guards keep
        // the phases pinned (storage before contracts/behaviour; invariants before
        // handlers).
        let mut store_fields: Vec<StoreField> = Vec::new();
        let mut invariants = Vec::new();
        let mut transitions = Vec::new();
        let mut migrations: Vec<MigrateDecl> = Vec::new();
        let mut handlers = Vec::new();
        // #1756: comments (and an orphaned doc block) before the closing `}`.
        let trailing_comments: Vec<Comment>;
        loop {
            let (mut leading, item_doc) = self.collect_item_lead();
            let storage_closed = !invariants.is_empty()
                || !transitions.is_empty()
                || !migrations.is_empty()
                || !handlers.is_empty();
            match self.peek_kind() {
                Some(TokenKind::RBrace) => {
                    if let Some(doc) = item_doc {
                        let doc_span = doc.span;
                        self.warnings.push(CompileError::new(
                            "bynk.parse.orphan_doc_block",
                            doc_span,
                            "documentation block has no following declaration to attach to",
                        ));
                        keep_orphan(&mut leading, doc);
                    }
                    trailing_comments = leading;
                    break;
                }
                // `store` is a contextual keyword (like `key`): the literal
                // identifier `store` in agent-body item position introduces a
                // store field, so it stays usable as an ordinary identifier
                // elsewhere (e.g. a `cache.store` context).
                Some(TokenKind::Ident) if self.peek_is_store_kw() => {
                    if storage_closed {
                        return Err(self.storage_after_phase_err());
                    }
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    // Store fields print with no blank line between them, so a
                    // comment's blank line goes (#1884). The invariants,
                    // transitions and handlers below are set apart and keep it.
                    leading.retain(|c| *c != Comment::Blank);
                    let mut sf = self.parse_store_field()?;
                    sf.documentation = doc;
                    sf.trivia.leading = leading;
                    sf.trivia.trailing = self.take_trailing_trivia();
                    store_fields.push(sf);
                }
                Some(TokenKind::Invariant) => {
                    if !handlers.is_empty() {
                        let t = self.peek().unwrap();
                        return Err(CompileError::new(
                            "bynk.parse.invariant_after_handler",
                            t.span,
                            "an `invariant` must be declared before the agent's handlers",
                        )
                        .with_note(
                            "invariants form a phase between the storage fields and the \
                             `on` handlers; move this invariant above the first handler",
                        ));
                    }
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    let mut inv = self.parse_invariant()?;
                    inv.documentation = doc;
                    inv.trivia.leading = leading;
                    inv.trivia.trailing = self.take_trailing_trivia();
                    invariants.push(inv);
                }
                // State-migrations S2 (#2224): `migrate` is contextual, like
                // `store`, and sits in the phase beside invariants.
                Some(TokenKind::Ident) if self.peek_is_migrate_kw() => {
                    if !handlers.is_empty() {
                        let t = self.peek().unwrap();
                        return Err(CompileError::new(
                            "bynk.parse.migrate_after_handler",
                            t.span,
                            "a `migrate` step must be declared before the agent's handlers",
                        )
                        .with_note(
                            "migration steps form a phase between the storage fields and the \
                             `on` handlers (beside `invariant`); move this step above the first \
                             handler",
                        ));
                    }
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    let mut m = self.parse_migrate()?;
                    m.documentation = doc;
                    m.trivia.leading = leading;
                    m.trivia.trailing = self.take_trailing_trivia();
                    migrations.push(m);
                }
                Some(TokenKind::Transition) => {
                    if !handlers.is_empty() {
                        let t = self.peek().unwrap();
                        return Err(CompileError::new(
                            "bynk.parse.transition_after_handler",
                            t.span,
                            "a `transition` must be declared before the agent's handlers",
                        )
                        .with_note(
                            "step invariants form a phase between the storage fields and the \
                             `on` handlers (beside `invariant`); move this transition above the \
                             first handler",
                        ));
                    }
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    let mut tr = self.parse_transition()?;
                    tr.documentation = doc;
                    tr.trivia.leading = leading;
                    tr.trivia.trailing = self.take_trailing_trivia();
                    transitions.push(tr);
                }
                // Handler-position annotations before an agent handler parse
                // uniformly (v0.140); validation rejects `@cache` on a non-HTTP
                // handler, keeping the grammar permissive and the diagnostic precise.
                Some(TokenKind::At) => {
                    let annotations = self.parse_handler_annotations()?;
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    let mut h = self.parse_handler(true, annotations)?;
                    h.documentation = doc;
                    h.trivia.leading = leading;
                    h.trivia.trailing = self.take_trailing_trivia();
                    handlers.push(h);
                }
                Some(TokenKind::On) => {
                    let next_span = self.peek().unwrap().span;
                    let doc = self.finalize_doc(item_doc, next_span, &mut leading);
                    let mut h = self.parse_handler(true, Vec::new())?;
                    h.documentation = doc;
                    h.trivia.leading = leading;
                    h.trivia.trailing = self.take_trailing_trivia();
                    handlers.push(h);
                }
                Some(_) => {
                    let t = self.peek().unwrap();
                    return Err(CompileError::new(
                        "bynk.parse.expected_handler",
                        t.span,
                        format!(
                            "expected `on` to start a handler, found {}",
                            t.kind.describe()
                        ),
                    ));
                }
                None => {
                    return Err(CompileError::new(
                        "bynk.parse.unexpected_eof",
                        self.eof_span(),
                        "expected `}` to close the agent body, found end of file",
                    ));
                }
            }
        }
        let close = self.expect(TokenKind::RBrace, "to close the agent body")?;
        if store_fields.is_empty() {
            return Err(CompileError::new(
                "bynk.parse.expected_agent_storage",
                kw.span.merge(close.span),
                "an agent must declare its storage — it has no `store` fields",
            ));
        }
        if handlers.is_empty() {
            return Err(CompileError::new(
                "bynk.parse.empty_agent",
                kw.span.merge(close.span),
                "an agent must declare at least one handler",
            ));
        }
        Ok(AgentDecl {
            trailing_comments,
            name,
            replaces,
            key_name,
            key_type,
            key_trivia,
            store_fields,
            invariants,
            transitions,
            migrations,
            handlers,
            documentation: None,
            span: kw.span.merge(close.span),
            trivia: Trivia::default(),
        })
    }

    /// The error for a `state`/`store` declaration appearing after the agent's
    /// invariants or handlers (the storage phase must come first).
    fn storage_after_phase_err(&self) -> CompileError {
        let t = self.peek().unwrap();
        CompileError::new(
            "bynk.parse.storage_after_phase",
            t.span,
            "agent storage (`state` / `store`) must be declared before the invariants and handlers",
        )
        .with_note("the agent body is ordered: key → storage → invariants → handlers")
    }

    /// Parse a single `store` field (v0.81): `store <name>: <Kind>[…] [= init]`.
    /// The kind is an ordinary type reference (`Cell[Int]`, `Map[K, V]`); the
    /// checker restricts which heads are storage kinds. Doc/trivia are attached
    /// by the caller. Access-pattern annotations are deferred (storage track Q3).
    /// True when the next token is the contextual keyword `store` (an identifier
    /// literally spelled `store`), introducing a storage-kind field.
    fn peek_is_migrate_kw(&self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Ident && self.slice(t.span) == "migrate")
    }

    /// Parse a migration step (state-migrations S2, #2224):
    /// `migrate from N [(old: T)] { item* }`, where an item is `f := e`,
    /// `let x = e` or `drop f`. Doc/trivia are attached by the caller.
    fn parse_migrate(&mut self) -> Result<MigrateDecl, CompileError> {
        let keyword = self.expect_ident("to start a `migrate` step")?.span;
        self.expect(TokenKind::From, "after `migrate`")?;
        let (from, from_span) = match self.peek() {
            Some(t) if t.kind == TokenKind::IntLit => {
                self.bump();
                let slice = self.slice(t.span);
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
                (n, t.span)
            }
            _ => {
                let span = self.peek().map(|t| t.span).unwrap_or(keyword);
                return Err(CompileError::new(
                    "bynk.parse.expected_token",
                    span,
                    "expected the schema version a `migrate` step reads (`migrate from 1`)",
                ));
            }
        };
        let old = if self.eat(TokenKind::LParen).is_some() {
            let name = self.expect_ident("as the old-state parameter name")?;
            self.expect(TokenKind::Colon, "after the old-state parameter name")?;
            let type_ref = self.parse_type_ref("as the old state's record type")?;
            self.expect(TokenKind::RParen, "to close the `migrate` parameter")?;
            Some(MigrateOld { name, type_ref })
        } else {
            None
        };
        self.expect(TokenKind::LBrace, "to open the `migrate` body")?;
        let mut items = Vec::new();
        let trailing_comments: Vec<Comment>;
        loop {
            let leading = self.take_leading_trivia();
            if self.peek_kind() == Some(TokenKind::RBrace) {
                trailing_comments = leading;
                break;
            }
            if self.assign_ahead() || self.peek_kind() == Some(TokenKind::Let) {
                let stmt = self.parse_statement()?;
                let trailing = self.take_trailing_trivia();
                match stmt {
                    Statement::Assign(mut a) => {
                        a.trivia.leading = leading;
                        a.trivia.trailing = trailing;
                        items.push(MigrateItem::Assign(a));
                    }
                    Statement::Let(mut l) => {
                        l.trivia.leading = leading;
                        l.trivia.trailing = trailing;
                        items.push(MigrateItem::Let(l));
                    }
                    other => {
                        return Err(CompileError::new(
                            "bynk.parse.migrate_item",
                            other.span(),
                            "a `migrate` step's body holds only `field := value`, `let` and \
                             `drop field`",
                        ));
                    }
                }
                continue;
            }
            if matches!(self.peek(), Some(t) if t.kind == TokenKind::Ident && self.slice(t.span) == "drop")
            {
                let kw = self.expect_ident("to start a `drop`")?.span;
                let field = self.expect_ident("as the field `drop` discards")?;
                let span = kw.merge(field.span);
                let trailing = self.take_trailing_trivia();
                items.push(MigrateItem::Drop(MigrateDrop {
                    keyword: kw,
                    field,
                    span,
                    trivia: Trivia { leading, trailing },
                }));
                continue;
            }
            let span = self.peek().map(|t| t.span).unwrap_or(keyword);
            return Err(CompileError::new(
                "bynk.parse.migrate_item",
                span,
                "expected `field := value`, `let` or `drop field` in a `migrate` step",
            ));
        }
        let close = self.expect(TokenKind::RBrace, "to close the `migrate` body")?;
        Ok(MigrateDecl {
            keyword,
            from,
            from_span,
            old,
            items,
            trailing_comments,
            documentation: None,
            span: keyword.merge(close.span),
            trivia: Trivia::default(),
        })
    }

    fn peek_is_store_kw(&self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Ident && self.slice(t.span) == "store")
    }

    fn parse_store_field(&mut self) -> Result<StoreField, CompileError> {
        let kw = self.expect_ident("to start a `store` field")?;
        let name = self.expect_ident("as the store field name after `store`")?;
        self.expect(TokenKind::Colon, "after the store field name")?;
        let kind = self.parse_store_kind()?;
        let mut end = kind.span;
        // v0.85 (ADR 0111): zero or more `@name(args)` annotations sit between the
        // storage kind and the `=` initialiser. The name is any identifier (the
        // checker matches it against the closed registry), so an unknown
        // annotation is a checker diagnostic rather than a parse error.
        let mut annotations = Vec::new();
        while self.peek_kind() == Some(TokenKind::At) {
            let ann = self.parse_annotation()?;
            end = ann.span;
            annotations.push(ann);
        }
        let init = if self.eat(TokenKind::Eq).is_some() {
            let e = self.parse_expr()?;
            end = e.span;
            Some(e)
        } else {
            None
        };
        Ok(StoreField {
            name,
            kind,
            annotations,
            init,
            documentation: None,
            span: kw.span.merge(end),
            trivia: Trivia::default(),
        })
    }

    /// Consume a run of one or more handler-position annotations (v0.140, ADR
    /// 0163) — `@cache(maxAge: 5.minutes) @…` — sitting between a handler's doc
    /// block and its `on`. Reuses [`parse_annotation`] (shared with `store`
    /// fields, ADR 0111); the caller has already confirmed the leading `@`. The
    /// next token must open a handler — a dangling annotation with no `on` is a
    /// parse error, so the annotation surface can never silently attach to nothing.
    fn parse_handler_annotations(&mut self) -> Result<Vec<Annotation>, CompileError> {
        let mut annotations = Vec::new();
        while self.peek_kind() == Some(TokenKind::At) {
            annotations.push(self.parse_annotation()?);
        }
        if self.peek_kind() != Some(TokenKind::On) {
            let span = self
                .peek()
                .map(|t| t.span)
                .unwrap_or_else(|| self.eof_span());
            let found = self
                .peek()
                .map(|t| t.kind.describe())
                .unwrap_or("end of file");
            return Err(CompileError::new(
                "bynk.parse.dangling_handler_annotation",
                span,
                format!("expected `on` to start the annotated handler, found {found}"),
            )
            .with_note(
                "a handler annotation (e.g. `@cache(maxAge: 5.minutes)`) must sit immediately before an `on` handler",
            ));
        }
        Ok(annotations)
    }

    /// Parse one storage annotation (v0.85; ADR 0111): `@<name>` or
    /// `@<name>(<arg>, …)`. Each argument is an optional `label:` then a value
    /// expression (`by: orderId`, `5.minutes`). The empty `@name()` form is
    /// accepted and yields no arguments. `pub(crate)` (not just this module's
    /// own carriers) since `parse_event_decl` (`parser/types.rs`, slice 3b,
    /// #978) reuses it from a sibling module the same way `messages`,
    /// `store` fields, and handlers already do from this one.
    pub(crate) fn parse_annotation(&mut self) -> Result<Annotation, CompileError> {
        let at = self.expect(TokenKind::At, "to start a storage annotation")?;
        let name = self.expect_ident("as the annotation name after `@`")?;
        let mut end = name.span;
        let mut args = Vec::new();
        if self.eat(TokenKind::LParen).is_some() {
            if self.peek_kind() != Some(TokenKind::RParen) {
                loop {
                    args.push(self.parse_annotation_arg()?);
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                }
            }
            let close =
                self.expect(TokenKind::RParen, "to close the annotation's argument list")?;
            end = close.span;
        }
        Ok(Annotation {
            name,
            args,
            span: at.span.merge(end),
        })
    }

    /// Parse one annotation argument: an optional `label:` prefix then a value
    /// expression. A word directly followed by `:` is a label (`by: orderId`);
    /// anything else is a positional value (`5.minutes`). The label word may be a
    /// reserved keyword (`by` is one), so it is taken by lexeme rather than via
    /// [`expect_ident`] — annotation labels are a closed metadata vocabulary, not
    /// expression identifiers.
    fn parse_annotation_arg(&mut self) -> Result<AnnotationArg, CompileError> {
        let label = if self.peek_annotation_label() {
            let tok = self.bump().unwrap();
            let label = Ident {
                name: self.slice(tok.span).to_string(),
                span: tok.span,
            };
            self.expect(TokenKind::Colon, "after an annotation argument label")?;
            Some(label)
        } else {
            None
        };
        let value = self.parse_expr()?;
        let span = label
            .as_ref()
            .map(|l| l.span.merge(value.span))
            .unwrap_or(value.span);
        Ok(AnnotationArg { label, value, span })
    }

    /// True when the cursor is on an alphabetic word immediately followed by `:`
    /// — the `label:` form of an annotation argument. Accepts keyword tokens
    /// (e.g. `by`) since labels are not expression identifiers.
    fn peek_annotation_label(&self) -> bool {
        let Some(tok) = self.peek() else { return false };
        if self.tokens.get(self.pos + 1).map(|t| t.kind) != Some(TokenKind::Colon) {
            return false;
        }
        self.slice(tok.span)
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
    }

    /// Consume a bare string-literal token, returning its parsed content
    /// (escapes applied, quotes stripped) and span. No expression parsing is
    /// involved — this is for grammar positions (a `messages` entry's code/
    /// template) that are always a plain string, never an interpolated one
    /// or any other expression form.
    fn expect_str_lit(&mut self, ctx: &str) -> Result<(String, Span), CompileError> {
        match self.peek() {
            Some(t) if t.kind == TokenKind::StrLit => {
                self.bump();
                Ok((parse_string_literal(self.slice(t.span), t.span)?, t.span))
            }
            Some(t) => Err(CompileError::new(
                "bynk.parse.expected_token",
                t.span,
                format!(
                    "expected a string literal {ctx}, found {}",
                    t.kind.describe()
                ),
            )),
            None => Err(CompileError::new(
                "bynk.parse.unexpected_eof",
                self.eof_span(),
                format!("expected a string literal {ctx}, found end of file"),
            )),
        }
    }

    /// #1883, slice 1a: `asset name = "path"`. The path is a plain string
    /// literal, never an interpolated one — it names a file at build time, so
    /// it must be known without evaluating anything. Placement and path
    /// safety are checker concerns; this parses identically wherever it is
    /// called from.
    fn parse_asset_decl(&mut self) -> Result<AssetDecl, CompileError> {
        let kw = self.expect(TokenKind::Asset, "to start an asset declaration")?;
        let name = self.expect_ident("after `asset`")?;
        self.expect(TokenKind::Eq, "after the asset name")?;
        let (path, path_span) = self.expect_str_lit("for the asset's file path")?;
        // #1883, slice 2: an optional `at "/path"`. `at` is contextual —
        // matched only here, lexed as an ordinary identifier — like `embeds`.
        // Nothing else can follow an asset's path on its line (every item
        // starts with a keyword), so this never misreads a following item.
        let at = if matches!(self.peek(), Some(t) if t.kind == TokenKind::Ident && self.slice(t.span) == "at")
        {
            self.bump();
            Some(self.expect_str_lit("for the asset's fixed URL path, after `at`")?)
        } else {
            None
        };
        let end = at.as_ref().map_or(path_span, |(_, span)| *span);
        Ok(AssetDecl {
            name,
            path,
            path_span,
            at,
            documentation: None,
            span: kw.span.merge(end),
            trivia: Trivia::default(),
            private: None,
        })
    }

    /// #2387, slice 1: `embed name = "path"`. The path is a plain string
    /// literal, never an interpolated one — it names a file at build time.
    /// Placement and path safety are checker concerns; this parses
    /// identically wherever it is called from.
    fn parse_embed_decl(&mut self) -> Result<EmbedDecl, CompileError> {
        let kw = self.expect(TokenKind::Embed, "to start an embed declaration")?;
        let name = self.expect_ident("after `embed`")?;
        self.expect(TokenKind::Eq, "after the embed name")?;
        let (path, path_span) = self.expect_str_lit("for the embedded file's path")?;
        Ok(EmbedDecl {
            name,
            path,
            path_span,
            documentation: None,
            span: kw.span.merge(path_span),
            trivia: Trivia::default(),
            private: None,
        })
    }

    /// #2617: `const Name: Type = <expr>`. The annotation is required: a
    /// constant is part of its unit's interface, and a reader (or a `uses`-ing
    /// unit) should see its type without inferring it, so a missing one is
    /// `bynk.parse.const_missing_type` rather than an inference. The name's
    /// case and the initialiser's static-ness are checker concerns; this
    /// parses identically wherever it is called from.
    fn parse_const_decl(&mut self) -> Result<ConstDecl, CompileError> {
        let kw = self.expect(TokenKind::Const, "to start a constant declaration")?;
        let name = self.expect_ident("after `const`")?;
        if let Some(t) = self.peek()
            && t.kind == TokenKind::Eq
        {
            return Err(CompileError::new(
                "bynk.parse.const_missing_type",
                name.span.merge(t.span),
                format!("constant `{}` needs a type annotation", name.name),
            )
            .with_note(format!(
                "write the type after the name: `const {}: Type = …`",
                name.name
            )));
        }
        self.expect(TokenKind::Colon, "after the constant name")?;
        let ty = self.parse_type_ref("for the constant's type")?;
        self.expect(TokenKind::Eq, "after the constant's type")?;
        let init = self.parse_expr()?;
        let span = kw.span.merge(init.span);
        Ok(ConstDecl {
            name,
            ty,
            init,
            documentation: None,
            span,
            trivia: Trivia::default(),
        })
    }

    /// message-bundles track, slice 1: `messages <tag> @reference { "code" =>
    /// "template" ... }` — a commons item declaring one locale's message
    /// bundle. `tag` is a plain identifier (its `LocaleTag` refinement is a
    /// checker concern, not a grammar one); annotations are zero-or-more here
    /// (cardinality — exactly one `@reference` per bundle — is validated
    /// downstream, per the same permissive-grammar/precise-checker split
    /// `@indexed`/`@ttl` already use, ADR 0111). Legality (commons-only) is
    /// likewise a checker concern, not a parser one — this function parses
    /// identically wherever it's called from.
    fn parse_messages_decl(&mut self) -> Result<MessagesDecl, CompileError> {
        let kw = self.expect(TokenKind::Messages, "to start a messages declaration")?;
        // A `LocaleTag` string literal, like an entry's `code`/`template` — so a
        // region/script tag (`"pt-BR"`) is expressible and the refinement is
        // checked, not lexed. Legality against `LocaleTag`'s pattern is
        // `check_messages_bundles`' concern (`bynk.messages.invalid_locale_tag`).
        let (tag, tag_span) = self.expect_str_lit("as the locale tag after `messages`")?;
        let mut annotations = Vec::new();
        while self.peek_kind() == Some(TokenKind::At) {
            annotations.push(self.parse_annotation()?);
        }
        self.expect(TokenKind::LBrace, "to open the messages body")?;
        let mut entries = Vec::new();
        while self.peek_kind() != Some(TokenKind::RBrace) {
            // #1888: an entry may carry a doc above it. Entries keep no
            // comments, so an orphaned doc before the `}` is dropped here; the
            // build warns about it and `bynk fmt`'s doc-loss guard refuses.
            let (_, documentation) = self.collect_member_lead(TokenKind::RBrace);
            if self.peek_kind() == Some(TokenKind::RBrace) {
                break;
            }
            let mut entry = self.parse_message_entry()?;
            entry.documentation = documentation;
            entries.push(entry);
            let _ = self.eat(TokenKind::Comma);
        }
        let close = self.expect(TokenKind::RBrace, "to close the messages body")?;
        Ok(MessagesDecl {
            tag,
            tag_span,
            annotations,
            entries,
            documentation: None,
            span: kw.span.merge(close.span),
            trivia: Trivia::default(),
            private: None,
        })
    }

    /// One `"code" => "template"` entry inside a `messages` body. Both sides
    /// are plain string literals — a template's `{name}` placeholders are
    /// resolved by a compile-time string scan during lowering, not parsed
    /// here (Decision D, message-bundles slice 1).
    fn parse_message_entry(&mut self) -> Result<MessageEntry, CompileError> {
        let (code, code_span) = self.expect_str_lit("as a message code")?;
        self.expect(TokenKind::FatArrow, "after a message code")?;
        let (template, template_span) = self.expect_str_lit("as a message template")?;
        Ok(MessageEntry {
            code,
            code_span,
            template,
            template_span,
            span: code_span.merge(template_span),
            documentation: None,
        })
    }

    /// Parse a storage kind applied to its element type(s): `Cell[Int]`,
    /// `Map[K, V]`, or a bare head. The head is any identifier; the checker
    /// restricts it to the closed kind catalogue.
    fn parse_store_kind(&mut self) -> Result<StoreKind, CompileError> {
        let head = self.expect_ident("as the storage kind (e.g. `Cell`, `Map`, `Log`)")?;
        let head_span = head.span;
        let mut end = head_span;
        let mut args = Vec::new();
        if self.eat(TokenKind::LBracket).is_some() {
            loop {
                args.push(self.parse_type_ref("as a storage-kind type argument")?);
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }
            let close = self.expect(
                TokenKind::RBracket,
                "to close the storage-kind type arguments",
            )?;
            end = close.span;
        }
        Ok(StoreKind {
            head,
            args,
            span: head_span.merge(end),
        })
    }

    /// Parse a single invariant declaration (v0.80): `invariant <name>: <expr>`.
    /// The predicate is an ordinary expression (with `implies`/`is`) over the
    /// agent's state fields; well-formedness is the checker's job. Doc and
    /// trivia are attached by the caller (the inline-declaration doc convention).
    fn parse_invariant(&mut self) -> Result<Invariant, CompileError> {
        let kw = self.expect(TokenKind::Invariant, "to start an invariant declaration")?;
        let name = self.expect_ident("as the invariant name after `invariant`")?;
        self.expect(TokenKind::Colon, "after the invariant name")?;
        let predicate = self.parse_expr()?;
        let span = kw.span.merge(predicate.span);
        Ok(Invariant {
            name,
            predicate,
            documentation: None,
            span,
            trivia: Trivia::default(),
        })
    }

    /// Parse a step invariant: `transition <name>: <predicate>` (v0.116, testing
    /// track slice 4). Mirrors [`parse_invariant`], but the predicate ranges over
    /// the contextual `old`/`new` state pair rather than the bare state fields.
    fn parse_transition(&mut self) -> Result<Transition, CompileError> {
        let kw = self.expect(TokenKind::Transition, "to start a transition declaration")?;
        let name = self.expect_ident("as the transition name after `transition`")?;
        self.expect(TokenKind::Colon, "after the transition name")?;
        let predicate = self.parse_expr()?;
        let span = kw.span.merge(predicate.span);
        Ok(Transition {
            name,
            predicate,
            documentation: None,
            span,
            trivia: Trivia::default(),
        })
    }

    /// Parse a handler block.
    ///
    /// Service handlers are `on call(args) -> T [by A] [given C1, C2] { body }`.
    /// Agent handlers are `on call methodName(args) -> T [by A] [given C1, C2] { body }`,
    /// where the method name is the agent operation invoked on an instance.
    /// Parse one handler, `on <form>(…) (params) -> T [by …] [given …] { body }`
    /// (v0.155: the `by` clause moved from before the parameter list to after the
    /// return type, next to `given`, killing the `by Actor (params)` call illusion).
    /// Any handler-position annotations (`@cache(…)`, v0.140) were consumed by the
    /// caller from the token stream *before* the `on` and are threaded in here — the
    /// grammar accepts them uniformly, and project validation gates each to its
    /// legal position (e.g. `@cache` only on `on GET`).
    fn parse_handler(
        &mut self,
        is_agent: bool,
        annotations: Vec<Annotation>,
    ) -> Result<Handler, CompileError> {
        let kw = self.expect(TokenKind::On, "to start a handler")?;
        // v0.44: the handler form is a single ident after `on` — `call`, an
        // HTTP method-builder (`GET("/route")`), `schedule("expr")`, or
        // `message(...)`. The protocol lives on the service header; the checker
        // verifies the form matches it.
        let kind_ident = self.expect_ident(
            "expected a handler form (e.g. `call`, `GET`, `schedule`, `message`) after `on`",
        )?;
        let kind = match kind_ident.name.as_str() {
            "call" => HandlerKind::Call,
            "message" => HandlerKind::Message,
            // v0.103: the WebSocket upgrade handler — `on open by … (params) …`.
            "open" => HandlerKind::Open,
            // v0.106 (slice 3b-iii): the WebSocket close handler. (`on message` on a
            // `from websocket` service reuses `HandlerKind::Message`; the checker
            // disambiguates the queue and WebSocket forms by the service protocol.)
            "close" => HandlerKind::Close,
            // Events track, slice 0 (spine #936): `on event(e: E)` — a
            // `from Events(E)` subscriber's one handler.
            "event" => HandlerKind::Event,
            // #2341 (decision C): a `from http` service's rendering of the
            // router's own rejections.
            "rejected" => HandlerKind::Rejected,
            "schedule" => {
                self.expect(TokenKind::LParen, "before the cron schedule expression")?;
                let expr_tok = self.expect(
                    TokenKind::StrLit,
                    "expected a cron expression string literal in `schedule(\"…\")`",
                )?;
                let expr = parse_string_literal(self.slice(expr_tok.span), expr_tok.span)?;
                self.expect(TokenKind::RParen, "to close the schedule expression")?;
                HandlerKind::Cron { expr }
            }
            method if HttpMethod::from_ident(method).is_some() => {
                let method = HttpMethod::from_ident(method).unwrap();
                self.expect(TokenKind::LParen, "before the route pattern")?;
                let path_tok = self.expect(
                    TokenKind::StrLit,
                    "expected a route pattern string literal in `GET(\"…\")`",
                )?;
                let path = parse_string_literal(self.slice(path_tok.span), path_tok.span)?;
                self.expect(TokenKind::RParen, "to close the route pattern")?;
                HandlerKind::Http { method, path }
            }
            other => {
                return Err(CompileError::new(
                    "bynk.parse.unknown_handler_kind",
                    kind_ident.span,
                    format!(
                        "unknown handler form `{other}` — expected `call`, an HTTP method (`GET`/`POST`/`PUT`/`PATCH`/`DELETE`), `rejected`, `schedule`, or `message`"
                    ),
                )
                .with_note(
                    "use `on call(...)`, `on GET(\"/path\") (...)`, `on schedule(\"expr\") (...)`, or `on message(m: T)`",
                ));
            }
        };
        // Only `on call` handlers are valid inside an agent.
        if is_agent && !matches!(kind, HandlerKind::Call) {
            return Err(CompileError::new(
                "bynk.parse.handler_in_agent",
                kind_ident.span,
                "only `on call` handlers are valid inside an `agent`; protocol handlers belong on a `service`",
            )
            .with_note(
                "agents persist state and respond to `on call`; HTTP routes, schedules, and queue messages belong on services",
            ));
        }
        // Agent handlers have a method name before the parameter list:
        //   on call addItem(item: CartItem) -> ...
        // Service handlers have just the parameter list:
        //   on call(amount: Money) -> ...
        let method_name = if is_agent && self.peek_kind() == Some(TokenKind::Ident) {
            Some(self.expect_ident("as the agent handler operation name")?)
        } else {
            None
        };
        self.expect(TokenKind::LParen, "before the handler parameter list")?;
        let mut params = Vec::new();
        if self.peek_kind() != Some(TokenKind::RParen) {
            params.push(self.parse_param()?);
            while self.eat(TokenKind::Comma).is_some() {
                params.push(self.parse_param()?);
            }
        }
        self.expect(TokenKind::RParen, "to close the handler parameter list")?;
        self.expect(TokenKind::Arrow, "before the handler return type")?;
        let return_type = self.parse_type_ref("as the handler return type")?;
        // v0.155: the optional `by`/`given` ambient clauses follow the return type,
        // `by` first. (`by` moved here from before the parameter list to end the
        // `by Actor (params)` call illusion.) Both are optional and independent.
        let by_clause = self.parse_by_clause()?;
        let given = self.parse_given_clause()?;
        let body = self.parse_block("to open the handler body")?;
        let span = kw.span.merge(body.span);
        // The annotation span (when any) extends the handler's start so hover /
        // go-to-definition over the `@name` resolves to this handler.
        let span = match annotations.first() {
            Some(first) => first.span.merge(body.span),
            None => span,
        };
        Ok(Handler {
            kind,
            annotations,
            method_name,
            by_clause,
            params,
            return_type,
            given,
            body,
            documentation: None,
            span,
            trivia: Trivia::default(),
        })
    }

    /// Parse the optional `by (<binder>:)? <Actor> (| <Actor>)*` clause (v0.45/
    /// v0.50/v0.52), returning `None` when the next token is not `by`. `by <name>:
    /// <Actor>` captures the verified identity (read as `name.identity`);
    /// `by <Actor>` declares-and-verifies the contract without capturing it
    /// (anonymous / verify-and-discard). One-token lookahead on `:` disambiguates.
    /// A `|`-separated list names an ordered sum of peer actors resolved first-wins
    /// (the binder requirement for a sum is the semantic rule
    /// `bynk.actor.sum_requires_binder`, not a parse one).
    ///
    /// Shared by handlers (after the return type, v0.155) and the service header
    /// (the service-level default, v0.155).
    fn parse_by_clause(&mut self) -> Result<Option<ByClause>, CompileError> {
        if self.peek_kind() != Some(TokenKind::By) {
            return Ok(None);
        }
        let by_kw = self.expect(TokenKind::By, "to start the actor clause")?;
        // `_` is not a valid binder — guide to the binder-less form.
        if self.peek_kind() == Some(TokenKind::Underscore) {
            let t = self.peek().unwrap();
            return Err(CompileError::new(
                "bynk.parse.expected_token",
                t.span,
                "`_` is not a valid actor binder".to_string(),
            )
            .with_note("omit the binder for an anonymous handler — `by <Actor>`"));
        }
        let first = self.expect_ident("as the actor (or its binder) after `by`")?;
        // A `:` after the first name means it was the binder; otherwise the first
        // name *is* the actor (binder-less form).
        let (binder, mut actors) = if self.peek_kind() == Some(TokenKind::Colon) {
            self.bump();
            (
                Some(first),
                vec![self.expect_ident("as the actor contract name after `:`")?],
            )
        } else {
            (None, vec![first])
        };
        while self.eat(TokenKind::Pipe).is_some() {
            actors.push(self.expect_ident("as a peer actor after `|`")?);
        }
        let last = actors.last().unwrap();
        let span = by_kw.span.merge(last.span);
        Ok(Some(ByClause {
            binder,
            actors,
            span,
        }))
    }

    /// Parse the optional `given C1, C2, …` capability clause (v0.12/v0.15),
    /// returning an empty `Vec` when the next token is not `given`. Shared by
    /// handlers (after the `by` clause, v0.155) and the service header (the
    /// service-level default, v0.155).
    fn parse_given_clause(&mut self) -> Result<Vec<CapRef>, CompileError> {
        let mut given = Vec::new();
        if self.peek_kind() == Some(TokenKind::Given) {
            self.bump();
            given.push(self.parse_cap_ref()?);
            while self.eat(TokenKind::Comma).is_some() {
                given.push(self.parse_cap_ref()?);
            }
        }
        Ok(given)
    }
}

/// A parsed `cors`/`security`/`limits` policy body, before it becomes the
/// policy's own field type ([`Parser::parse_policy_body`]).
struct PolicyBody {
    fields: Vec<PolicyField>,
    trailing_comments: Vec<Comment>,
    close: Span,
}

/// One `name: value` field of a policy body, with its comments.
struct PolicyField {
    name: Ident,
    value: Expr,
    span: Span,
    trivia: Trivia,
}

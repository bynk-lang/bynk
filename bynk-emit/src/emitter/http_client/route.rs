//! One route's text: its `__route_*` constant, its client method and the argument
//! encodings the method needs. Split from `http_client.rs` (1A.38b) so each file
//! stays under the module-size ceiling; the code is unchanged.

use std::collections::HashMap;
use std::sync::Arc;

use bynk_check::contract;
use bynk_check::project_model::TypeDecl;
use bynk_check::wire::{TypeBody, TypeRef};
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt,
    TsType, TsTypeMember,
};

use super::*;

impl Gen {
    // -----------------------------------------------------------------
    // One route
    // -----------------------------------------------------------------

    pub(super) fn route(
        &mut self,
        service: &str,
        r: &RouteInput,
        rp: &RoutePlan,
        types: &HashMap<String, Arc<TypeDecl>>,
    ) -> Result<RouteText, ClientError> {
        let ident = format!("__route_{}_{}", sanitise(service), rp.function);
        let hash = contract::http_route_contract_hash(&Self::contract_of(r), types);
        let success = r.success.as_ref().map(|t| self.renamed(t));
        let replies = self.replies(r, &success)?;

        // The response union.
        let mut members: Vec<TsType> = Vec::new();
        let mut raw_done: Vec<u16> = Vec::new();
        for (status, variant, payload) in &replies {
            if payload.is_raw() {
                // The wire cannot tell one raw answer from another at a status, so they
                // are one member, named by every variant that can make it.
                if raw_done.contains(status) {
                    continue;
                }
                raw_done.push(*status);
                members.push(self.raw_member(*status, &replies));
                continue;
            }
            let mut fields = vec![
                TsTypeMember::readonly_prop("status", TsType::named(status.to_string())),
                TsTypeMember::readonly_prop("kind", TsType::named(format!("\"{variant}\""))),
            ];
            match payload {
                ReplyPayload::Value => {
                    let t = success.as_ref().expect("a value reply has a success type");
                    fields.push(TsTypeMember::readonly_prop("body", self.public_type(t)?));
                }
                ReplyPayload::Message => {
                    fields.push(TsTypeMember::readonly_prop(
                        "error",
                        TsType::named("string"),
                    ));
                }
                ReplyPayload::Location => {
                    fields.push(TsTypeMember::readonly_prop(
                        "location",
                        TsType::named("string"),
                    ));
                }
                ReplyPayload::BoundaryError => {
                    fields.push(TsTypeMember::readonly_prop(
                        "error",
                        TsType::named("BoundaryError"),
                    ));
                }
                ReplyPayload::BoundaryRefusal => {
                    fields.push(TsTypeMember::readonly_prop(
                        "error",
                        TsType::Object(vec![
                            TsTypeMember::readonly_prop(
                                "kind",
                                TsType::named(format!("\"{variant}\"")),
                            ),
                            TsTypeMember::readonly_prop("details", TsType::named("string")),
                        ]),
                    ));
                }
                ReplyPayload::Events => {
                    fields.push(TsTypeMember::readonly_prop(
                        "events",
                        TsType::named("globalThis.AsyncIterable<string>"),
                    ));
                }
                ReplyPayload::None => {}
                ReplyPayload::Raw | ReplyPayload::RawEtag | ReplyPayload::RawTagged => {
                    unreachable!("a raw reply is built by `raw_member`")
                }
            }
            members.push(TsType::Object(fields));
        }
        let response_type = if members.is_empty() {
            None
        } else {
            let mut d = TsStmt::decl(
                TsDecl::Export(Box::new(TsDecl::TypeAlias {
                    name: rp.response.clone(),
                    type_params: Vec::new(),
                    ty: TsType::multiline_union(members),
                })),
                None,
            );
            d.no_blank_before = true;
            Some(vec![
                TsStmt::doc_comment(
                    format!("What `{} {}` answers, by `status`.", r.method, r.path),
                    None,
                ),
                d,
            ])
        };

        // The route constant: its replies and how to decode them.
        let mut by_status: BTreeMap<u16, Vec<TsExpr>> = BTreeMap::new();
        let mut raw_seen: Vec<u16> = Vec::new();
        for (status, variant, payload) in &replies {
            if payload.is_raw() {
                if raw_seen.contains(status) {
                    continue;
                }
                raw_seen.push(*status);
                let group: Vec<&ReplyPayload> = replies
                    .iter()
                    .filter(|(s, _, p)| s == status && p.is_raw())
                    .map(|(_, _, p)| p)
                    .collect();
                let mut fields = vec![("kind", str_lit(variant)), ("body", str_lit("raw"))];
                match Self::raw_etag(&group) {
                    RawEtag::None => {}
                    RawEtag::Maybe => fields.push(("etag", str_lit("maybe"))),
                    RawEtag::Required => fields.push(("etag", str_lit("required"))),
                }
                by_status
                    .entry(*status)
                    .or_default()
                    .push(object_of(fields));
                continue;
            }
            let entry = match payload {
                ReplyPayload::Value => {
                    let t = success.clone().expect("a value reply has a success type");
                    let decode = serialisation::deserialise_ref_via(&t, "", &self.ru);
                    let converts = self.holds_option(&t);
                    let conv = self.convert(&t, Dir::ToPublic, id("v"), 0)?;
                    let decode = if converts {
                        // `(json) => __then(decode(json, "$"), (v) => conv)`
                        arrow(
                            vec!["json"],
                            call(
                                "__then",
                                vec![
                                    TsExpr::Call {
                                        callee: Box::new(decode),
                                        args: vec![id("json"), str_lit("$")],
                                    },
                                    arrow(vec!["v"], conv),
                                ],
                            ),
                        )
                    } else {
                        decode
                    };
                    object_of(vec![
                        ("kind", str_lit(variant)),
                        ("body", str_lit("value")),
                        ("decode", decode),
                    ])
                }
                other => {
                    let body = match other {
                        ReplyPayload::Message => "message",
                        ReplyPayload::Location => "location",
                        ReplyPayload::BoundaryError | ReplyPayload::BoundaryRefusal => "boundary",
                        ReplyPayload::Events => "events",
                        _ => "none",
                    };
                    object_of(vec![("kind", str_lit(variant)), ("body", str_lit(body))])
                }
            };
            by_status.entry(*status).or_default().push(entry);
        }
        let mut constant_entries = vec![
            TsObjectEntry::prop("method", str_lit(&r.method)),
            TsObjectEntry::prop("path", str_lit(&r.path)),
            TsObjectEntry::prop("contract", str_lit(&hash)),
            TsObjectEntry::prop(
                "replies",
                TsExpr::multiline_object_entries(
                    by_status
                        .into_iter()
                        .map(|(status, entries)| {
                            TsObjectEntry::prop(status.to_string(), TsExpr::array(entries))
                        })
                        .collect(),
                ),
            ),
        ];
        if replies
            .iter()
            .any(|(_, _, p)| p.is_raw() || *p == ReplyPayload::Events)
        {
            constant_entries.push(TsObjectEntry::prop("streamed", id("__streamed")));
        }
        let constant = TsStmt::decl(
            TsDecl::ConstDecl {
                name: ident.clone(),
                ty: Some(TsType::named("__Route")),
                init: TsExpr::multiline_object_entries(constant_entries),
            },
            None,
        );

        // The function: its parameters and the encoding of each.
        let (mut params, build, arg_types) = self.encoding(r, rp)?;
        let init =
            if r.params.iter().any(
                |p| matches!(p, ParamInput::Path { name, .. } if super::super::ts_ident(name) == "init"),
            ) {
                "init_"
            } else {
                "init"
            };
        params.push(TsParam::optional(
            init,
            Some(TsType::named("globalThis.RequestInit")),
        ));
        let ret = if replies.is_empty() {
            TsType::named("CallFailure")
        } else {
            TsType::union(vec![
                TsType::named(rp.response.clone()),
                TsType::named("CallFailure"),
            ])
        };
        let promise = TsType::named_with_args("globalThis.Promise", vec![ret]);
        let call = TsExpr::Call {
            callee: Box::new(id("__call")),
            args: vec![
                id("options"),
                id(init),
                id(&ident),
                TsExpr::Arrow {
                    params: Vec::new(),
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(TsArrowBody::Multiline(build)),
                },
            ],
        };
        let mut method = TsObjectEntry::prop(
            property(&rp.function),
            TsExpr::Arrow {
                params,
                is_async: false,
                generics: Vec::new(),
                return_type: Some(promise.clone()),
                body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::As {
                    expr: Box::new(call),
                    ty: promise,
                }))),
            },
        );
        if let Some(doc) = r.doc.as_deref() {
            method = method.with_doc(doc);
        }
        Ok(RouteText {
            hash,
            constant,
            method,
            response_type,
            arg_types,
        })
    }

    /// The one union member for the raw answers at `status`: the body left to the
    /// caller, as a stream with `bytes()` and `text()` over the same body.
    fn raw_member(&self, status: u16, replies: &[(u16, String, ReplyPayload)]) -> TsType {
        let group: Vec<&(u16, String, ReplyPayload)> = replies
            .iter()
            .filter(|(s, _, p)| *s == status && p.is_raw())
            .collect();
        let kinds = group
            .iter()
            .map(|(_, v, _)| format!("\"{v}\""))
            .collect::<Vec<_>>()
            .join(" | ");
        let payloads: Vec<&ReplyPayload> = group.iter().map(|(_, _, p)| p).collect();
        let mut fields = vec![
            TsTypeMember::readonly_prop("status", TsType::named(status.to_string())),
            TsTypeMember::readonly_prop("kind", TsType::named(kinds)),
            TsTypeMember::readonly_prop("contentType", TsType::named("string")),
        ];
        match Self::raw_etag(&payloads) {
            RawEtag::None => {}
            RawEtag::Maybe => fields.push(TsTypeMember::readonly_prop(
                "etag",
                TsType::named("string | null"),
            )),
            RawEtag::Required => {
                fields.push(TsTypeMember::readonly_prop("etag", TsType::named("string")))
            }
        }
        fields.push(TsTypeMember::readonly_prop(
            "body",
            TsType::named("globalThis.ReadableStream<globalThis.Uint8Array> | null"),
        ));
        fields.push(TsTypeMember::readonly_prop(
            "bytes",
            TsType::named("() => globalThis.Promise<globalThis.Uint8Array>"),
        ));
        fields.push(TsTypeMember::readonly_prop(
            "text",
            TsType::named("() => globalThis.Promise<string>"),
        ));
        TsType::Object(fields)
    }

    /// Whether the raw answers of one status carry an `ETag` the caller can rely on.
    fn raw_etag(group: &[&ReplyPayload]) -> RawEtag {
        if group.iter().all(|p| matches!(p, ReplyPayload::RawTagged)) {
            RawEtag::Required
        } else if group
            .iter()
            .any(|p| matches!(p, ReplyPayload::RawEtag | ReplyPayload::RawTagged))
        {
            RawEtag::Maybe
        } else {
            RawEtag::None
        }
    }

    /// What a client of `r` acts on, for its contract hash.
    fn contract_of(r: &RouteInput) -> contract::HttpRouteContract {
        let params = r
            .params
            .iter()
            .map(|p| match p {
                ParamInput::Path { name, ty } => contract::HttpRouteParam {
                    kind: "path".to_string(),
                    name: name.clone(),
                    ty: Some(ty.clone()),
                    detail: None,
                },
                ParamInput::Query { ty } => contract::HttpRouteParam {
                    kind: "query".to_string(),
                    name: "query".to_string(),
                    ty: Some(ty.clone()),
                    detail: None,
                },
                ParamInput::Cookies { ty } => contract::HttpRouteParam {
                    kind: "cookies".to_string(),
                    name: "cookies".to_string(),
                    ty: Some(ty.clone()),
                    detail: None,
                },
                ParamInput::Body { ty, kind } => {
                    let (kind, ty, detail) = match kind {
                        BodyKind::Json => ("body-json", Some(ty.clone()), None),
                        BodyKind::Form => ("body-form", Some(ty.clone()), None),
                        BodyKind::Multipart => ("body-multipart", Some(ty.clone()), None),
                        BodyKind::Raw { content_type } => (
                            "body-raw",
                            None,
                            Some(content_type.clone().unwrap_or_else(|| "*/*".to_string())),
                        ),
                    };
                    contract::HttpRouteParam {
                        kind: kind.to_string(),
                        name: "body".to_string(),
                        ty,
                        detail,
                    }
                }
            })
            .collect();
        contract::HttpRouteContract {
            method: r.method.clone(),
            path: r.path.clone(),
            params,
            success: r.success.clone(),
            replies: r
                .replies
                .iter()
                .map(|x| (x.status, x.variant.clone(), x.payload.name().to_string()))
                .collect(),
        }
    }

    /// The replies as `(status, kind, payload)`, in a stable order, none twice.
    fn replies(
        &self,
        r: &RouteInput,
        success: &Option<TypeRef>,
    ) -> Result<Vec<(u16, String, ReplyPayload)>, ClientError> {
        let mut out: Vec<(u16, String, ReplyPayload)> = Vec::new();
        for reply in &r.replies {
            let payload = match reply.payload {
                ReplyPayload::Value if success.is_none() => ReplyPayload::None,
                p => p,
            };
            let item = (reply.status, reply.variant.clone(), payload);
            if !out.contains(&item) {
                out.push(item);
            }
        }
        out.sort_by(|a, b| (a.0, a.2, &a.1).cmp(&(b.0, b.2, &b.1)));
        Ok(out)
    }

    /// The parameter list, the body of the encoding thunk and the types of the
    /// arguments the route takes beyond the boundary's own (`query`, `cookies`).
    fn encoding(&mut self, r: &RouteInput, rp: &RoutePlan) -> Result<Encoding, ClientError> {
        let mut signature: Vec<TsParam> = Vec::new();
        let mut build: Vec<TsStmt> = Vec::new();
        let mut params: Vec<TsObjectEntry> = Vec::new();
        let mut entries: Vec<TsObjectEntry> = Vec::new();
        let mut arg_types: Vec<TsStmt> = Vec::new();
        for p in &r.params {
            match p {
                ParamInput::Path { name, ty } => {
                    let ty = self.renamed(ty);
                    let ident = super::super::ts_ident(name);
                    signature.push(TsParam::new(ident.clone(), Some(self.public_type(&ty)?)));
                    let checked = match &ty {
                        TypeRef::Base(..) => call("globalThis.String", vec![id(&ident)]),
                        _ => {
                            let ser =
                                serialisation::serialise_expr_via(&ty, &id(&ident), "", &self.ru);
                            let de = serialisation::deserialise_ref_via(&ty, "", &self.ru);
                            let local = format!("__p_{ident}");
                            build.extend(checked_stmts(&local, ser, de, &format!("path.{name}")));
                            call("globalThis.String", vec![member(id(&local), "value")])
                        }
                    };
                    params.push(TsObjectEntry::prop(format!("\"{name}\""), checked));
                }
                ParamInput::Body {
                    ty,
                    kind: BodyKind::Json,
                } => {
                    let ty = self.renamed(ty);
                    signature.push(TsParam::new("body", Some(self.public_type(&ty)?)));
                    let wired = self.convert(&ty, Dir::ToWire, id("body"), 0)?;
                    let ser = serialisation::serialise_expr_via(&ty, &wired, "", &self.ru);
                    let de = serialisation::deserialise_ref_via(&ty, "", &self.ru);
                    build.extend(checked_stmts("__b", ser, de, "$"));
                    entries.push(TsObjectEntry::prop("body", member(id("__b"), "value")));
                }
                ParamInput::Body {
                    ty,
                    kind: BodyKind::Form,
                } => {
                    let ty = self.renamed(ty);
                    signature.push(TsParam::new("body", Some(self.public_type(&ty)?)));
                    let arg = self.record_arg(&ty, "body", "body", None)?;
                    build.extend(arg.stmts);
                    entries.push(TsObjectEntry::prop(
                        "form",
                        call(
                            "__formString",
                            vec![id("__w_body"), TsExpr::array(arg.specs)],
                        ),
                    ));
                }
                ParamInput::Body {
                    ty,
                    kind: BodyKind::Multipart,
                } => {
                    let ty = self.renamed(ty);
                    signature.push(TsParam::new("body", Some(self.multipart_type(&ty)?)));
                    build.extend(self.multipart_arg(&ty)?);
                    entries.push(TsObjectEntry::prop("multipart", id("__form")));
                }
                ParamInput::Body {
                    kind: BodyKind::Raw { content_type },
                    ..
                } => {
                    signature.push(TsParam::new(
                        "body",
                        Some(TsType::union(vec![
                            TsType::named("globalThis.Uint8Array"),
                            TsType::named("globalThis.ArrayBuffer"),
                            TsType::named("globalThis.Blob"),
                            TsType::named("globalThis.ReadableStream<globalThis.Uint8Array>"),
                        ])),
                    ));
                    entries.push(TsObjectEntry::prop("raw", id("body")));
                    if let Some(ct) = content_type {
                        entries.push(TsObjectEntry::prop("rawType", str_lit(ct)));
                    }
                }
                ParamInput::Query { ty } => {
                    let ty = self.renamed(ty);
                    let name = rp.query.clone().expect("a query route has a query type");
                    let arg = self.record_arg(&ty, "query", "query", Some(&name))?;
                    build.extend(arg.stmts.iter().cloned());
                    entries.push(TsObjectEntry::prop(
                        "query",
                        call(
                            "__queryString",
                            vec![id("__w_query"), TsExpr::array(arg.specs.clone())],
                        ),
                    ));
                    signature.push(arg.parameter("query", &name));
                    arg_types.extend(arg.decl);
                }
                ParamInput::Cookies { ty } => {
                    let ty = self.renamed(ty);
                    let name = rp
                        .cookies
                        .clone()
                        .expect("a cookies route has a cookies type");
                    let arg = self.record_arg(&ty, "cookies", "cookies", Some(&name))?;
                    build.extend(arg.stmts.iter().cloned());
                    entries.push(TsObjectEntry::prop(
                        "cookies",
                        call(
                            "__cookieString",
                            vec![id("__w_cookies"), TsExpr::array(arg.specs.clone())],
                        ),
                    ));
                    signature.push(arg.parameter("cookies", &name));
                    arg_types.extend(arg.decl);
                }
            }
        }
        entries.insert(
            0,
            TsObjectEntry::prop("params", TsExpr::object_entries(params)),
        );
        build.push(TsStmt::return_stmt(
            Some(call("Ok", vec![TsExpr::object_entries(entries)])),
            None,
        ));
        Ok((signature, build, arg_types))
    }

    /// The type of a `@multipart` body: the record's public type, its file parts
    /// `Blob`s.
    fn multipart_type(&self, record: &TypeRef) -> Result<TsType, ClientError> {
        self.public_type(record)
    }

    /// The statements that write a `@multipart` body: its text fields and file parts
    /// into a `FormData`, after the record's own decoder has checked the fields (a
    /// file part stands in as an empty file, since its bytes are the caller's).
    fn multipart_arg(&mut self, record: &TypeRef) -> Result<Vec<TsStmt>, ClientError> {
        let TypeRef::Named(id_) = record else {
            return fail("a `@multipart` body is a record");
        };
        let Some(decl) = self.sc.types.get(&id_.name).cloned() else {
            return fail("the `@multipart` record is not declared");
        };
        let TypeBody::Record(rec) = &decl.body else {
            return fail("a `@multipart` body is a record");
        };
        let mut stmts = vec![
            wire_object_stmt("body"),
            const_of(
                "__form",
                None,
                TsExpr::New {
                    callee: Box::new(member(id("globalThis"), "FormData")),
                    args: Vec::new(),
                },
            ),
        ];
        let mut parts: Vec<TsStmt> = Vec::new();
        let mut specs = Vec::new();
        for f in &rec.fields {
            let fname = f.name.name.clone();
            let wire = f.wire_name().unwrap_or_else(|| fname.clone());
            let (inner, list, option) = match &f.type_ref {
                TypeRef::List(i, _) => (i.as_ref(), true, false),
                TypeRef::Option(i, _) => (i.as_ref(), false, true),
                other => (other, false, false),
            };
            let is_file = matches!(inner, TypeRef::Named(i) if self.is_file(&i.name));
            let field = || member(id("body"), &fname);
            if !is_file {
                let wired = self.convert(&f.type_ref, Dir::ToWire, field(), 0)?;
                let ser = serialisation::serialise_expr_via(&f.type_ref, &wired, "", &self.ru);
                stmts.push(wire_assign("body", &fname, ser));
                specs.push(field_spec(&fname, f.wire_name()));
                continue;
            }
            // A file part: an empty file stands in for the decoder, and the blob is appended.
            let stub = || {
                object_of(vec![
                    ("filename", object_of(vec![("kind", str_lit("None"))])),
                    ("contentType", str_lit("")),
                    ("bytes", str_lit("")),
                ])
            };
            let append = |part: TsExpr| {
                TsStmt::expr_stmt(
                    TsExpr::Call {
                        callee: Box::new(member(id("__form"), "append")),
                        args: vec![str_lit(&wire), part.clone(), call("__fileName", vec![part])],
                    },
                    None,
                )
            };
            let (check, appended) = match (list, option) {
                (true, _) => (
                    TsExpr::Call {
                        callee: Box::new(member(field(), "map")),
                        args: vec![TsExpr::Arrow {
                            params: Vec::new(),
                            is_async: false,
                            generics: Vec::new(),
                            return_type: None,
                            body: Box::new(TsArrowBody::Expr(Box::new(stub()))),
                        }],
                    },
                    TsStmt::for_of("part", field(), append(id("part")), None),
                ),
                (_, true) => {
                    let present = TsExpr::Binary {
                        op: TsBinaryOp::And,
                        left: Box::new(compare(
                            TsBinaryOp::StrictNotEq,
                            field(),
                            TsExpr::Lit(TsLit::Null),
                        )),
                        right: Box::new(compare(TsBinaryOp::StrictNotEq, field(), id("undefined"))),
                    };
                    let absent = TsExpr::Binary {
                        op: TsBinaryOp::Or,
                        left: Box::new(compare(
                            TsBinaryOp::StrictEq,
                            field(),
                            TsExpr::Lit(TsLit::Null),
                        )),
                        right: Box::new(compare(TsBinaryOp::StrictEq, field(), id("undefined"))),
                    };
                    (
                        TsExpr::Conditional {
                            test: Box::new(absent),
                            consequent: Box::new(object_of(vec![("kind", str_lit("None"))])),
                            alternate: Box::new(object_of(vec![
                                ("kind", str_lit("Some")),
                                ("value", stub()),
                            ])),
                        },
                        TsStmt::if_stmt(present, append(field()), None),
                    )
                }
                _ => (stub(), append(field())),
            };
            stmts.push(wire_assign("body", &fname, check));
            parts.push(appended);
        }
        let de = serialisation::deserialise_ref_via(record, "", &self.ru);
        stmts.extend(checked_stmts("__c_body", id("__w_body"), de, "body"));
        stmts.push(TsStmt::expr_stmt(
            call(
                "__appendText",
                vec![
                    id("__form"),
                    call("__formString", vec![id("__w_body"), TsExpr::array(specs)]),
                ],
            ),
            None,
        ));
        stmts.extend(parts);
        Ok(stmts)
    }

    /// A record argument (`query`, `cookies` or a `@form` body): each field written
    /// into its wire form, then checked by the record's own decoder, so a
    /// refinement is refused before sending and a default is the server's to fill.
    /// With a `custom` type name the argument is its own interface, where a field that
    /// is an `Option` or has a default may be left out; without one the argument is
    /// the record's public type and every field is given.
    fn record_arg(
        &mut self,
        record: &TypeRef,
        arg: &str,
        at: &str,
        custom: Option<&str>,
    ) -> Result<RecordArg, ClientError> {
        let TypeRef::Named(id_) = record else {
            return fail(format!("a `{arg}` parameter is a record"));
        };
        let Some(decl) = self.sc.types.get(&id_.name).cloned() else {
            return fail(format!("the `{arg}` record `{}` is not declared", id_.name));
        };
        let TypeBody::Record(rec) = &decl.body else {
            return fail(format!("a `{arg}` parameter is a record"));
        };
        let mut members = Vec::new();
        let mut stmts = vec![wire_object_stmt(arg)];
        let mut specs = Vec::new();
        let mut all_optional = true;
        for f in &rec.fields {
            let fname = f.name.name.clone();
            let optional =
                custom.is_some() && (matches!(f.type_ref, TypeRef::Option(..)) || f.init.is_some());
            all_optional &= optional;
            members.push(TsTypeMember::Prop {
                name: fname.clone(),
                ty: self.public_type(&f.type_ref)?,
                optional,
                readonly: true,
            });
            let given = member(id(arg), &fname);
            let wired = self.convert(&f.type_ref, Dir::ToWire, given.clone(), 0)?;
            let ser = serialisation::serialise_expr_via(&f.type_ref, &wired, "", &self.ru);
            let write = wire_assign(arg, &fname, ser);
            if optional {
                stmts.push(TsStmt::if_stmt(
                    compare(TsBinaryOp::StrictNotEq, given, id("undefined")),
                    write,
                    None,
                ));
            } else {
                stmts.push(write);
            }
            specs.push(field_spec(&fname, f.wire_name()));
        }
        let de = serialisation::deserialise_ref_via(record, "", &self.ru);
        stmts.extend(checked_stmts(
            &format!("__c_{arg}"),
            id(&format!("__w_{arg}")),
            de,
            at,
        ));
        let declared = custom.map(|name| {
            let mut d = TsStmt::decl(
                TsDecl::Export(Box::new(TsDecl::Interface {
                    name: name.to_string(),
                    type_params: Vec::new(),
                    members,
                })),
                None,
            );
            d.no_blank_before = true;
            vec![
                TsStmt::doc_comment(
                    if arg == "query" {
                        "The `query` of the call: a field left out is absent from the query string."
                            .to_string()
                    } else {
                        "The `cookies` of the call, sent as its `Cookie` header: a field left out is\nabsent. **For a server-side call** (a framework's server, a Worker, Node). A\nbrowser sets its own `Cookie` header and drops one a script writes, so a call\nmade where `document` exists with cookies given returns `CallFailure` `Invalid`\nrather than lose them; a page uses `credentials: \"include\"` instead.".to_string()
                    },
                    None,
                ),
                d,
            ]
        });
        Ok(RecordArg {
            decl: declared.into_iter().flatten().collect(),
            stmts,
            specs,
            all_optional,
        })
    }
}

/// A route's parameters, the statements of its encoding thunk and the argument types it declares.
type Encoding = (Vec<TsParam>, Vec<TsStmt>, Vec<TsStmt>);

/// A record argument's type, its encoding and what that needs.
struct RecordArg {
    /// The argument's own interface, when it has one.
    decl: Vec<TsStmt>,
    /// The statements that write and check its wire form.
    stmts: Vec<TsStmt>,
    /// The `QueryField`s that name each field's wire key.
    specs: Vec<TsExpr>,
    /// Whether every field may be left out, so the argument may be.
    all_optional: bool,
}

impl RecordArg {
    /// The parameter, `name: Type`, or `name: Type = {}` when it may be left out.
    fn parameter(&self, name: &str, ty: &str) -> TsParam {
        let param = TsParam::new(name, Some(TsType::named(ty)));
        if self.all_optional {
            param.with_default(TsExpr::object(Vec::new()))
        } else {
            param
        }
    }
}

// ---------------------------------------------------------------------
// Small node builders
// ---------------------------------------------------------------------

fn id(name: &str) -> TsExpr {
    TsExpr::Ident(name.to_string())
}

fn str_lit(text: &str) -> TsExpr {
    TsExpr::Lit(TsLit::Str(text.to_string()))
}

fn member(object: TsExpr, property: &str) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.to_string(),
    }
}

fn call(callee: &str, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(id(callee)),
        args,
    }
}

/// `(a, b) => body`.
fn arrow(params: Vec<&str>, body: TsExpr) -> TsExpr {
    TsExpr::Arrow {
        params: params.into_iter().map(|p| TsParam::new(p, None)).collect(),
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(body))),
    }
}

/// A one-line object literal of `key: value` entries.
fn object_of(fields: Vec<(&str, TsExpr)>) -> TsExpr {
    TsExpr::object(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn compare(op: TsBinaryOp, left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn const_of(name: &str, ty: Option<TsType>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.to_string()), ty, init, None)
}

/// `const __w_<arg>: { [k: string]: JsonValue } = {};`
fn wire_object_stmt(arg: &str) -> TsStmt {
    const_of(
        &format!("__w_{arg}"),
        Some(TsType::Object(vec![TsTypeMember::index(
            "k",
            TsType::named("string"),
            TsType::named("JsonValue"),
        )])),
        TsExpr::object(Vec::new()),
    )
}

/// `__w_<arg>["field"] = value;`
fn wire_assign(arg: &str, field: &str, value: TsExpr) -> TsStmt {
    TsStmt::assign(
        TsExpr::Index {
            object: Box::new(id(&format!("__w_{arg}"))),
            index: Box::new(str_lit(field)),
        },
        value,
        None,
    )
}

/// `{ name: "field", wire: "wire" }`, the `QueryField` of one field.
fn field_spec(field: &str, wire: Option<String>) -> TsExpr {
    let mut fields = vec![("name", str_lit(field))];
    if let Some(w) = wire {
        fields.push(("wire", str_lit(&w)));
    }
    object_of(fields)
}

/// `const <local> = __checked(value, decoder, "at"); if (<local>.tag === "Err") return <local>;`
fn checked_stmts(local: &str, value: TsExpr, decoder: TsExpr, at: &str) -> Vec<TsStmt> {
    vec![
        const_of(
            local,
            None,
            call("__checked", vec![value, decoder, str_lit(at)]),
        ),
        TsStmt::if_stmt(
            compare(
                TsBinaryOp::StrictEq,
                member(id(local), "tag"),
                str_lit("Err"),
            ),
            TsStmt::return_stmt(Some(id(local)), None),
            None,
        ),
    ]
}

pub(super) struct RouteText {
    /// The route's contract hash.
    pub(super) hash: String,
    pub(super) constant: TsStmt,
    pub(super) method: TsObjectEntry,
    pub(super) response_type: Option<Vec<TsStmt>>,
    pub(super) arg_types: Vec<TsStmt>,
}

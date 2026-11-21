//! One route's text: its `__route_*` constant, its client method and the argument
//! encodings the method needs. Split from `http_client.rs` (1A.38b) so each file
//! stays under the module-size ceiling; the code is unchanged.

use std::collections::HashMap;
use std::sync::Arc;

use bynk_check::contract;
use bynk_check::project_model::TypeDecl;
use bynk_check::wire::{TypeBody, TypeRef};
use bynk_ts::{TsDecl, TsStmt, TsType, TsTypeMember};

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
        let mut by_status: BTreeMap<u16, Vec<String>> = BTreeMap::new();
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
                let etag = match Self::raw_etag(&group) {
                    RawEtag::None => "",
                    RawEtag::Maybe => ", etag: \"maybe\"",
                    RawEtag::Required => ", etag: \"required\"",
                };
                by_status
                    .entry(*status)
                    .or_default()
                    .push(format!("{{ kind: \"{variant}\", body: \"raw\"{etag} }}"));
                continue;
            }
            let entry = match payload {
                ReplyPayload::Value => {
                    let t = success.clone().expect("a value reply has a success type");
                    let decode = serialisation::deserialise_ref_via(&t, "", &self.ru);
                    let decode = bynk_ts::print_expr(&decode);
                    let conv = self.convert(&t, Dir::ToPublic, "v", 0)?;
                    let decode = if conv == "v" {
                        decode
                    } else {
                        format!("(json) => __then({decode}(json, \"$\"), (v) => {conv})")
                    };
                    format!("{{ kind: \"{variant}\", body: \"value\", decode: {decode} }}")
                }
                other => {
                    let body = match other {
                        ReplyPayload::Message => "message",
                        ReplyPayload::Location => "location",
                        ReplyPayload::BoundaryError | ReplyPayload::BoundaryRefusal => "boundary",
                        ReplyPayload::Events => "events",
                        _ => "none",
                    };
                    format!("{{ kind: \"{variant}\", body: \"{body}\" }}")
                }
            };
            by_status.entry(*status).or_default().push(entry);
        }
        let mut constant = format!(
            "const {ident}: __Route = {{\n  method: \"{}\",\n  path: \"{}\",\n  contract: \"{hash}\",\n  replies: {{\n",
            r.method, r.path
        );
        for (status, entries) in &by_status {
            constant.push_str(&format!("    {status}: [{}],\n", entries.join(", ")));
        }
        constant.push_str("  },\n");
        if replies
            .iter()
            .any(|(_, _, p)| p.is_raw() || *p == ReplyPayload::Events)
        {
            constant.push_str("  streamed: __streamed,\n");
        }
        constant.push_str("};\n\n");

        // The function: its parameters and the encoding of each.
        let (signature, build, arg_types) = self.encoding(r, rp)?;
        let init =
            if r.params.iter().any(
                |p| matches!(p, ParamInput::Path { name, .. } if super::super::ts_ident(name) == "init"),
            ) {
                "init_"
            } else {
                "init"
            };
        let ret = if replies.is_empty() {
            "CallFailure".to_string()
        } else {
            format!("{} | CallFailure", rp.response)
        };
        let doc = r
            .doc
            .as_deref()
            .map(|d| {
                let mut text = String::from("      /**\n");
                for line in d.lines() {
                    text.push_str(
                        &format!("       * {}\n", line.trim_end()).replace(" * \n", " *\n"),
                    );
                }
                text.push_str("       */\n");
                text
            })
            .unwrap_or_default();
        let method = format!(
            "{doc}      {}: ({signature}{sep}{init}?: globalThis.RequestInit): globalThis.Promise<{ret}> =>\n        __call(options, {init}, {ident}, () => {{\n{build}        }}) as globalThis.Promise<{ret}>,\n",
            property(&rp.function),
            sep = if signature.is_empty() { "" } else { ", " },
        );
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
    fn encoding(
        &mut self,
        r: &RouteInput,
        rp: &RoutePlan,
    ) -> Result<(String, String, Vec<TsStmt>), ClientError> {
        let mut signature: Vec<String> = Vec::new();
        let mut build = String::new();
        let mut params = Vec::new();
        let mut entries: Vec<String> = Vec::new();
        let mut arg_types: Vec<TsStmt> = Vec::new();
        for p in &r.params {
            match p {
                ParamInput::Path { name, ty } => {
                    let ty = self.renamed(ty);
                    let id = super::super::ts_ident(name);
                    signature.push(format!("{id}: {}", self.public_text(&ty)?));
                    let checked = match &ty {
                        TypeRef::Base(..) => format!("globalThis.String({id})"),
                        _ => {
                            let ser = bynk_ts::print_expr(&serialisation::serialise_expr_via(
                                &ty,
                                &bynk_ts::TsExpr::Ident(id.clone()),
                                "",
                                &self.ru,
                            ));
                            let de = bynk_ts::print_expr(&serialisation::deserialise_ref_via(
                                &ty, "", &self.ru,
                            ));
                            build.push_str(&format!(
                                "          const __p_{id} = __checked({ser}, {de}, \"path.{name}\");\n          if (__p_{id}.tag === \"Err\") return __p_{id};\n"
                            ));
                            format!("globalThis.String(__p_{id}.value)")
                        }
                    };
                    params.push(format!("\"{name}\": {checked}"));
                }
                ParamInput::Body {
                    ty,
                    kind: BodyKind::Json,
                } => {
                    let ty = self.renamed(ty);
                    signature.push(format!("body: {}", self.public_text(&ty)?));
                    let wired = self.convert(&ty, Dir::ToWire, "body", 0)?;
                    let ser = bynk_ts::print_expr(&serialisation::serialise_expr_via(
                        &ty,
                        &bynk_ts::TsExpr::Ident(wired.clone()),
                        "",
                        &self.ru,
                    ));
                    let de =
                        bynk_ts::print_expr(&serialisation::deserialise_ref_via(&ty, "", &self.ru));
                    build.push_str(&format!(
                        "          const __b = __checked({ser}, {de}, \"$\");\n          if (__b.tag === \"Err\") return __b;\n"
                    ));
                    entries.push("body: __b.value".to_string());
                }
                ParamInput::Body {
                    ty,
                    kind: BodyKind::Form,
                } => {
                    let ty = self.renamed(ty);
                    signature.push(format!("body: {}", self.public_text(&ty)?));
                    let arg = self.record_arg(&ty, "body", "body", None)?;
                    build.push_str(&arg.text);
                    entries.push(format!("form: __formString(__w_body, [{}])", arg.specs));
                }
                ParamInput::Body {
                    ty,
                    kind: BodyKind::Multipart,
                } => {
                    let ty = self.renamed(ty);
                    signature.push(format!("body: {}", self.multipart_type(&ty)?));
                    let arg = self.multipart_arg(&ty)?;
                    build.push_str(&arg);
                    entries.push("multipart: __form".to_string());
                }
                ParamInput::Body {
                    kind: BodyKind::Raw { content_type },
                    ..
                } => {
                    signature.push(
                        "body: globalThis.Uint8Array | globalThis.ArrayBuffer | globalThis.Blob | globalThis.ReadableStream<globalThis.Uint8Array>"
                            .to_string(),
                    );
                    entries.push("raw: body".to_string());
                    if let Some(ct) = content_type {
                        entries.push(format!("rawType: {ct:?}"));
                    }
                }
                ParamInput::Query { ty } => {
                    let ty = self.renamed(ty);
                    let name = rp.query.clone().expect("a query route has a query type");
                    let arg = self.record_arg(&ty, "query", "query", Some(&name))?;
                    build.push_str(&arg.text);
                    entries.push(format!("query: __queryString(__w_query, [{}])", arg.specs));
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
                    build.push_str(&arg.text);
                    entries.push(format!(
                        "cookies: __cookieString(__w_cookies, [{}])",
                        arg.specs
                    ));
                    signature.push(arg.parameter("cookies", &name));
                    arg_types.extend(arg.decl);
                }
            }
        }
        entries.insert(0, format!("params: {{ {} }}", params.join(", ")));
        build.push_str(&format!(
            "          return Ok({{ {} }});\n",
            entries.join(", ")
        ));
        Ok((signature.join(", "), build, arg_types))
    }

    /// The type of a `@multipart` body: the record's public type, its file parts
    /// `Blob`s.
    fn multipart_type(&self, record: &TypeRef) -> Result<String, ClientError> {
        self.public_text(record)
    }

    /// The statements that write a `@multipart` body: its text fields and file parts
    /// into a `FormData`, after the record's own decoder has checked the fields (a
    /// file part stands in as an empty file, since its bytes are the caller's).
    fn multipart_arg(&mut self, record: &TypeRef) -> Result<String, ClientError> {
        let TypeRef::Named(id) = record else {
            return fail("a `@multipart` body is a record");
        };
        let Some(decl) = self.sc.types.get(&id.name).cloned() else {
            return fail("the `@multipart` record is not declared");
        };
        let TypeBody::Record(rec) = &decl.body else {
            return fail("a `@multipart` body is a record");
        };
        let mut text = String::from(
            "          const __w_body: { [k: string]: JsonValue } = {};\n          const __form = new globalThis.FormData();\n",
        );
        let mut parts = String::new();
        let mut specs = Vec::new();
        for f in &rec.fields {
            let fname = f.name.name.clone();
            let key = format!("{fname:?}");
            let wire = f.wire_name().unwrap_or_else(|| fname.clone());
            let wire_key = format!("{wire:?}");
            let (inner, list, option) = match &f.type_ref {
                TypeRef::List(i, _) => (i.as_ref(), true, false),
                TypeRef::Option(i, _) => (i.as_ref(), false, true),
                other => (other, false, false),
            };
            let is_file = matches!(inner, TypeRef::Named(i) if self.is_file(&i.name));
            if !is_file {
                let wired = self.convert(&f.type_ref, Dir::ToWire, &format!("body.{fname}"), 0)?;
                let ser = bynk_ts::print_expr(&serialisation::serialise_expr_via(
                    &f.type_ref,
                    &bynk_ts::TsExpr::Ident(wired.clone()),
                    "",
                    &self.ru,
                ));
                text.push_str(&format!("          __w_body[{key}] = {ser};\n"));
                let w = f
                    .wire_name()
                    .map(|w| format!(", wire: {w:?}"))
                    .unwrap_or_default();
                specs.push(format!("{{ name: {key}{w} }}"));
                continue;
            }
            // A file part: an empty file stands in for the decoder, and the blob is appended.
            let stub = "{ filename: { kind: \"None\" }, contentType: \"\", bytes: \"\" }";
            let (check, append) = match (list, option) {
                (true, _) => (
                    format!("body.{fname}.map(() => ({stub}))"),
                    format!(
                        "          for (const part of body.{fname}) __form.append({wire_key}, part, __fileName(part));\n"
                    ),
                ),
                (_, true) => (
                    format!(
                        "body.{fname} === null || body.{fname} === undefined ? {{ kind: \"None\" }} : {{ kind: \"Some\", value: {stub} }}"
                    ),
                    format!(
                        "          if (body.{fname} !== null && body.{fname} !== undefined) __form.append({wire_key}, body.{fname}, __fileName(body.{fname}));\n"
                    ),
                ),
                _ => (
                    stub.to_string(),
                    format!(
                        "          __form.append({wire_key}, body.{fname}, __fileName(body.{fname}));\n"
                    ),
                ),
            };
            text.push_str(&format!("          __w_body[{key}] = {check};\n"));
            parts.push_str(&append);
        }
        let de = bynk_ts::print_expr(&serialisation::deserialise_ref_via(record, "", &self.ru));
        text.push_str(&format!(
            "          const __c_body = __checked(__w_body, {de}, \"body\");\n          if (__c_body.tag === \"Err\") return __c_body;\n          __appendText(__form, __formString(__w_body, [{}]));\n",
            specs.join(", ")
        ));
        text.push_str(&parts);
        Ok(text)
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
        let TypeRef::Named(id) = record else {
            return fail(format!("a `{arg}` parameter is a record"));
        };
        let Some(decl) = self.sc.types.get(&id.name).cloned() else {
            return fail(format!("the `{arg}` record `{}` is not declared", id.name));
        };
        let TypeBody::Record(rec) = &decl.body else {
            return fail(format!("a `{arg}` parameter is a record"));
        };
        let mut members = Vec::new();
        let mut text = format!("          const __w_{arg}: {{ [k: string]: JsonValue }} = {{}};\n");
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
            let wired = self.convert(&f.type_ref, Dir::ToWire, &format!("{arg}.{fname}"), 0)?;
            let ser = bynk_ts::print_expr(&serialisation::serialise_expr_via(
                &f.type_ref,
                &bynk_ts::TsExpr::Ident(wired.clone()),
                "",
                &self.ru,
            ));
            let key = format!("{fname:?}");
            if optional {
                text.push_str(&format!(
                    "          if ({arg}.{fname} !== undefined) __w_{arg}[{key}] = {ser};\n"
                ));
            } else {
                text.push_str(&format!("          __w_{arg}[{key}] = {ser};\n"));
            }
            let wire = f
                .wire_name()
                .map(|w| format!(", wire: {w:?}"))
                .unwrap_or_default();
            specs.push(format!("{{ name: {key}{wire} }}"));
        }
        let de = bynk_ts::print_expr(&serialisation::deserialise_ref_via(record, "", &self.ru));
        text.push_str(&format!(
            "          const __c_{arg} = __checked(__w_{arg}, {de}, \"{at}\");\n          if (__c_{arg}.tag === \"Err\") return __c_{arg};\n"
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
            text,
            specs: specs.join(", "),
            all_optional,
        })
    }
}

/// A record argument's type, its encoding and what that needs.
struct RecordArg {
    /// The argument's own interface, when it has one.
    decl: Vec<TsStmt>,
    /// The statements that write and check its wire form.
    text: String,
    /// The `QueryField`s that name each field's wire key.
    specs: String,
    /// Whether every field may be left out, so the argument may be.
    all_optional: bool,
}

impl RecordArg {
    /// The parameter, `name: Type`, or `name: Type = {}` when it may be left out.
    fn parameter(&self, name: &str, ty: &str) -> String {
        if self.all_optional {
            format!("{name}: {ty} = {{}}")
        } else {
            format!("{name}: {ty}")
        }
    }
}

pub(super) struct RouteText {
    /// The route's contract hash.
    pub(super) hash: String,
    pub(super) constant: String,
    pub(super) method: String,
    pub(super) response_type: Option<Vec<TsStmt>>,
    pub(super) arg_types: Vec<TsStmt>,
}

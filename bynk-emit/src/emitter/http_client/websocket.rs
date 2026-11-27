//! #3906: a `from websocket` service's typed connection. One `connect` per service, on the same
//! client object as the HTTP routes, so it shares `baseUrl`, `origin` and the other options.
//!
//! Nothing here is a second implementation of the boundary: the frames cross the wire as the server's
//! own codecs read them, and the route parameters (the query string of the upgrade) are encoded and
//! checked as an HTTP path parameter is. What the runtime adds is the connection itself
//! (`__webSocket` in `runtime/src/client/http_client.ts`): the credential offered as the server reads it,
//! the frames checked both ways, and opt-in reconnection.

use bynk_check::actors::WsClientCredential;
use bynk_syntax::ast::BaseType;
use bynk_ts::TsArrowBody;

use super::route::{arrow, checked_stmts};
use super::*;
use crate::ts_build::{bool_lit, call, ident, member, str_lit};

/// What a `from websocket` service hands the generator: its frame types, the route parameters of
/// its `on open` and the credential its `by` clause reads.
#[derive(Debug, Clone)]
pub struct WsInput {
    /// The frame the client sends (`in`).
    pub in_ty: TypeRef,
    /// The frame the server sends (`out`).
    pub out_ty: TypeRef,
    /// `on open`'s route parameters, in order: the query string of the upgrade.
    pub params: Vec<(String, TypeRef)>,
    pub credential: WsClientCredential,
}

/// The names a service's `connect` declares.
pub(super) struct WsPlan {
    /// The credentials type, for a service that takes any.
    pub(super) credentials: Option<String>,
    /// The connection type.
    pub(super) connection: String,
}

/// A service's text.
pub(super) struct WsText {
    pub(super) constant: TsStmt,
    pub(super) types: Vec<TsStmt>,
    pub(super) method: TsObjectEntry,
}

/// Whether a frame type is exactly `Bytes`: a binary message, as the server reads it. Any other type
/// is JSON text, an opaque or refined `Bytes` included.
pub(super) fn is_plain_bytes(t: &TypeRef) -> bool {
    matches!(t, TypeRef::Base(BaseType::Bytes, _))
}

/// The type of a credential's token: a value, or a (possibly async) function read at each connect.
fn token_type() -> TsType {
    TsType::named("string | (() => string | globalThis.Promise<string>)")
}

impl Gen {
    /// A `from websocket` service's `__ws_<key>` spec, its credential and connection types, and its
    /// `connect`.
    pub(super) fn ws_service(
        &mut self,
        key: &str,
        s: &ServiceInput,
        w: &WsInput,
        wp: &WsPlan,
    ) -> Result<WsText, ClientError> {
        let in_ty = self.renamed(&w.in_ty);
        let out_ty = self.renamed(&w.out_ty);
        let (in_bytes, out_bytes) = (is_plain_bytes(&in_ty), is_plain_bytes(&out_ty));
        let (in_public, out_public) = (self.public_type(&in_ty)?, self.public_type(&out_ty)?);
        let json_value = TsType::named("JsonValue");
        // `encode`: the value as the server reads it, after the server's own checks.
        let encode = if in_bytes {
            arrow(vec!["frame"], call(ident("Ok"), vec![ident("frame")]))
        } else {
            let wired = self.convert(&in_ty, Dir::ToWire, ident("frame"), 0)?;
            let ser = serialisation::serialise_expr_via(&in_ty, &wired, "", &self.ru);
            let de = serialisation::deserialise_ref_via(&in_ty, "", &self.ru);
            let mut body = checked_stmts("__b", ser, de, "$");
            body.push(TsStmt::return_stmt(
                Some(call(ident("Ok"), vec![member(ident("__b"), "value")])),
                None,
            ));
            TsExpr::Arrow {
                params: vec![TsParam::new("frame", None)],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Multiline(body)),
            }
        };
        // `decode`: a frame the server sent, checked and in its public shape.
        let decode = if out_bytes {
            arrow(
                vec!["frame"],
                call(
                    ident("Ok"),
                    vec![TsExpr::As {
                        expr: Box::new(ident("frame")),
                        ty: TsType::named("globalThis.Uint8Array"),
                    }],
                ),
            )
        } else {
            let decoder = serialisation::deserialise_ref_via(&out_ty, "", &self.ru);
            let read = TsExpr::Call {
                callee: Box::new(decoder),
                args: vec![
                    TsExpr::As {
                        expr: Box::new(ident("frame")),
                        ty: json_value,
                    },
                    str_lit("$"),
                ],
            };
            if self.holds_option(&out_ty) {
                let conv = self.convert(&out_ty, Dir::ToPublic, ident("v"), 0)?;
                arrow(
                    vec!["frame"],
                    call(ident("__then"), vec![read, arrow(vec!["v"], conv)]),
                )
            } else {
                arrow(vec!["frame"], read)
            }
        };
        let credential = match w.credential {
            WsClientCredential::None => "none",
            WsClientCredential::Bearer { optional: false } => "bearer",
            WsClientCredential::Bearer { optional: true } => "bearer?",
            WsClientCredential::Oidc => "oidc",
            WsClientCredential::Cookie => "cookie",
        };
        let spec_ident = ["__ws_", &sanitise(key)].concat();
        let spec_type =
            TsType::named_with_args("__WsSpec", vec![in_public.clone(), out_public.clone()]);
        let constant = TsStmt::decl(
            TsDecl::ConstDecl {
                name: spec_ident.clone(),
                ty: Some(spec_type),
                init: TsExpr::multiline_object_entries(vec![
                    TsObjectEntry::prop("credential", str_lit(credential)),
                    TsObjectEntry::prop("binaryIn", bool_lit(in_bytes)),
                    TsObjectEntry::prop("binaryOut", bool_lit(out_bytes)),
                    TsObjectEntry::prop("encode", encode),
                    TsObjectEntry::prop("decode", decode),
                ]),
            },
            None,
        );

        // The route parameters, encoded and checked as an HTTP path parameter is.
        let route = RouteInput {
            method: "GET".to_string(),
            path: "/".to_string(),
            doc: None,
            params: w
                .params
                .iter()
                .map(|(name, ty)| ParamInput::Path {
                    name: name.clone(),
                    ty: ty.clone(),
                })
                .collect(),
            success: None,
            replies: Vec::new(),
        };
        let plan = RoutePlan {
            function: "connect".to_string(),
            response: String::new(),
            query: None,
            cookies: None,
            headers: None,
        };
        let (mut params, build, _) = self.encoding(&route, &plan)?;
        let mut taken: BTreeSet<String> = params.iter().map(|p| p.name.clone()).collect();
        taken.insert("options".to_string());

        // The credentials the actor reads, and the options of the connection.
        let mut types: Vec<TsStmt> = Vec::new();
        let mut credentials_arg: Option<String> = None;
        let mut credentials_ty = TsType::named("undefined");
        if let Some(name) = &wp.credentials {
            let (field, optional) = match w.credential {
                WsClientCredential::Bearer { optional } => ("bearer", optional),
                WsClientCredential::Oidc => ("oidc", false),
                WsClientCredential::None | WsClientCredential::Cookie => {
                    unreachable!("only a token credential is declared")
                }
            };
            let member = TsTypeMember::Prop {
                name: field.to_string(),
                ty: token_type(),
                optional,
                readonly: true,
            };
            types.extend(documented_type(
                name,
                match w.credential {
                    WsClientCredential::Oidc => "The credential `connect` offers: the identity provider's token, or a function that makes one, read at each connect and each reconnect.",
                    WsClientCredential::Bearer { .. } | WsClientCredential::None | WsClientCredential::Cookie => "The credential `connect` offers: the bearer token, or a function that makes one, read at each connect and each reconnect.",
                },
                TsType::Object(vec![member]),
            ));
            let arg = unique("credentials".to_string(), &mut taken);
            let ty = TsType::named(name.clone());
            let mut param = TsParam::new(arg.clone(), Some(ty.clone()));
            if optional {
                param = TsParam::optional(arg.clone(), Some(ty.clone()));
            }
            params.push(param);
            credentials_arg = Some(arg);
            credentials_ty = ty;
        }
        let connect_arg = unique("connect".to_string(), &mut taken);
        params.push(TsParam::optional(
            connect_arg.clone(),
            Some(TsType::named_with_args(
                "WsConnectOptions",
                vec![credentials_ty],
            )),
        ));
        let connection = TsType::named_with_args("WsConnection", vec![in_public, out_public]);
        types.extend(documented_type(
            &wp.connection,
            &[
                "The connection `",
                key,
                ".connect` opens: typed frames both ways.",
            ]
            .concat(),
            connection,
        ));
        let ret = TsType::union(vec![
            TsType::named(wp.connection.clone()),
            TsType::named("CallFailure"),
        ]);
        let opened = call(
            ident("__webSocket"),
            vec![
                ident("options"),
                ident(&spec_ident),
                TsExpr::Arrow {
                    params: Vec::new(),
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(TsArrowBody::Multiline(build)),
                },
                match &credentials_arg {
                    Some(a) => ident(a),
                    None => crate::emitter::internal_fault::undefined_node(),
                },
                ident(&connect_arg),
            ],
        );
        let mut method = TsObjectEntry::prop(
            "connect",
            TsExpr::Arrow {
                params,
                is_async: false,
                generics: Vec::new(),
                return_type: Some(ret.clone()),
                body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::As {
                    expr: Box::new(opened),
                    ty: ret,
                }))),
            },
        );
        let doc = s.doc.as_deref().map_or_else(
            || ["Open a connection to `", &s.name, "`."].concat(),
            |d| [d, "\n\nOpen a connection to `", &s.name, "`."].concat(),
        );
        method = method.with_doc(doc);
        Ok(WsText {
            constant,
            types,
            method,
        })
    }
}

/// `export type <name> = <ty>;` under its doc comment.
fn documented_type(name: &str, doc: &str, ty: TsType) -> Vec<TsStmt> {
    let mut d = TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::TypeAlias {
            name: name.to_string(),
            type_params: Vec::new(),
            ty,
        })),
        None,
    );
    d.no_blank_before = true;
    vec![TsStmt::doc_comment(doc.to_string(), None), d]
}

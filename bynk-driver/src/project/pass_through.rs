//! #3752 (pass-through HTTP, slice 3a): an `on ANY(…)` route type-checks, but its
//! router suspensions and the `Upstream` exchange arrive in slice 3b. Until then a
//! build that would emit one is refused, never built without its routes: a Worker
//! deployed without them would answer `404` where the program says it forwards.

use bynk_project::{AttributedError, ParsedFile};
use bynk_syntax::CompileError;
use bynk_syntax::ast::{CommonsItem, HandlerKind, HttpMethod};

/// The diagnostic for a pass-through route a build reaches.
pub(super) const NOT_EMITTED: &str = "bynk.http.pass_through_not_emitted";

/// One error per `on ANY(…)` route in `parsed`, attributed to its file.
pub(super) fn not_emitted_errors(parsed: &[ParsedFile]) -> Vec<AttributedError> {
    let mut out = Vec::new();
    for file in parsed {
        for item in file.items() {
            let CommonsItem::Service(service) = item else {
                continue;
            };
            for h in &service.handlers {
                if let HandlerKind::Http {
                    method: HttpMethod::Any,
                    path,
                } = &h.kind
                {
                    let error = CompileError::new(
                        NOT_EMITTED,
                        h.span,
                        format!("the pass-through route `ANY {path}` checks, but this release can't build it yet"),
                    )
                    .with_note("its router and the `Upstream` exchange arrive in the next increment; `bynk check` accepts it meanwhile");
                    out.push(AttributedError::new(Some(file.identity_path()), error));
                }
            }
        }
    }
    out
}

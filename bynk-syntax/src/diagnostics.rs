//! Central registry of diagnostic codes.
//!
//! This is the single source of truth for the `bynk.*` codes the compiler can
//! emit. The reference page `site/src/content/docs/book/reference/diagnostics.md` is generated
//! from [`render_markdown`], and the test `tests/diagnostics_registry.rs`
//! asserts that this table matches exactly the set of codes used across the
//! compiler source — so a new code cannot be introduced without documenting it
//! here, and a removed code cannot linger in the docs.
//!
//! Each entry is a `(code, summary)` pair, optionally tagged with the grammar
//! production(s) it constrains (`grammar_symbol`), and carries a severity
//! (`Error` unless built via the crate-private `warn` helper). The category
//! shown in the generated reference is derived from the second dotted segment
//! of the code; the grammar weave (`docs/grammar-semantics.json`, the
//! `{{#grammar-semantics}}` directive, and the diagnostics page's Construct
//! column) is generated from `grammar_symbol`.

use crate::error::Severity;

/// One documented diagnostic: its stable code and a one-line summary of the
/// cause. Richer "cause and fix" material for the common diagnostics lives in
/// the troubleshooting how-to guides.
pub struct DiagnosticInfo {
    pub code: &'static str,
    pub summary: &'static str,
    /// The grammar production(s) this diagnostic constrains, by `tree-sitter`
    /// rule name (e.g. `http_handler`). This is the single source of the
    /// "static semantics" weave: a grammar-reference entry embeds the
    /// diagnostics for a rule via `{{#grammar-semantics <rule>}}`, generated
    /// from here. Empty for diagnostics with no single governing construct
    /// (e.g. `bynk.boundary.structural_mismatch`). Every non-empty name is
    /// checked against the grammar by `tests/diagnostics_registry.rs`.
    pub grammar_symbol: &'static [&'static str],
    /// This code's severity (ADR 0117): `Error` rejects the program; `Warning`
    /// surfaces but never fails the build. The single source of truth for
    /// [`crate::error::Severity::for_error`], which looks a code up here
    /// instead of hardcoding its own copy of the (small) warning set —
    /// `tests/diagnostics_registry.rs` asserts the two never drift apart.
    /// Defaults to `Error` via the crate-private `d`/`dg` constructors; only
    /// the crate-private `warn` helper's call sites override it.
    pub severity: Severity,
}

/// The hosted Book, the stable target for `codeDescription` links (#853,
/// DECISION C). No trailing slash — an [`Explain::page`] path (which begins
/// with `/`) is appended directly.
pub const BOOK_BASE_URL: &str = "https://bynk-lang.org";

/// A curated, offline-complete explanation of a diagnostic code (#853).
///
/// This is the compiler-owned `code → { blurb, href }` table (DECISION A): the
/// `blurb`/`example` are the offline answer `bynk explain` prints, while
/// `page`/`anchor` compose the hosted-Book href the LSP hangs off the code as a
/// clickable `codeDescription` link (DECISION C). Only the highest-traffic,
/// newcomer-facing codes are curated; every other code simply has no entry and
/// falls back gracefully — no link, no error (DECISION B).
///
/// The mapping points at *existing* Book concept pages rather than duplicating
/// their prose; the reference-page generator ([`render_markdown`]) links every
/// explained code at its `page`/`anchor` as an in-site link, so
/// `astro build`'s link checker fails if a page moves or an anchor is renamed —
/// the doc-drift guard the rest of the site already relies on.
pub struct Explain {
    /// The diagnostic code this explains. Must be a real [`REGISTRY`] code
    /// (enforced by `tests/diagnostics_registry.rs`).
    pub code: &'static str,
    /// A longer-form paragraph: what the rule is and, crucially, *why* it
    /// exists. This is the offline-complete answer — useful without network.
    pub blurb: &'static str,
    /// A minimal example of the violation and its fix: the offending line is
    /// marked `✗` and the corrected one `✓` in a trailing comment. Written to be
    /// read, so it may elide with `…`; `bynk-tests`' `explain_examples.rs` holds
    /// every line of it to a compiled pair of whole programs (#3163).
    ///
    /// An example that spans files says which one each snippet is in with a
    /// comment-only line holding just a path: `// shop.bynk`, the file's path
    /// under the project's `src/` (`// shop/till.bynk` if nested), or
    /// `// bynk.toml`. The label applies until the next, and the gate holds each
    /// line under it to that file (the bad project's if marked `✗`, the good
    /// project's if `✓`, either's if unmarked). A label must name a file of the
    /// fixture, and only a directory-form fixture may use one.
    ///
    /// The gate also checks the layout of the string as it prints: a line
    /// inside a `{ … }` block must be indented past the line that opens it, and
    /// the closing `}` must sit at that line's indentation. A `\` line
    /// continuation swallows the whitespace that follows it, newlines included,
    /// so write a nested line's leading spaces as `\x20` (four spaces a level)
    /// and a blank line as `\n\`, never as a line holding only `\`.
    pub example: &'static str,
    /// The target Book page as a site-root-relative path, no extension and no
    /// trailing slash, e.g. `/book/reference/types`. Used verbatim as an
    /// in-site link by [`render_markdown`] (so the site link checker guards it)
    /// and prefixed with [`BOOK_BASE_URL`] for the hosted `codeDescription`.
    pub page: &'static str,
    /// The in-page heading anchor (slug), or `""` for the page top.
    pub anchor: &'static str,
}

impl Explain {
    /// The hosted-Book URL for this explanation: [`BOOK_BASE_URL`] + the page
    /// (Starlight serves pages with a trailing slash) + `#anchor` when set.
    pub fn href(&self) -> String {
        let mut url = format!("{BOOK_BASE_URL}{}/", self.page);
        if !self.anchor.is_empty() {
            url.push('#');
            url.push_str(self.anchor);
        }
        url
    }

    /// The site-root-relative link used inside the generated reference page,
    /// e.g. `/book/reference/types/#record-types`. Same shape as [`href`], sans
    /// the host, so the in-site link checker resolves it.
    ///
    /// [`href`]: Explain::href
    pub fn in_site_link(&self) -> String {
        let mut link = format!("{}/", self.page);
        if !self.anchor.is_empty() {
            link.push('#');
            link.push_str(self.anchor);
        }
        link
    }
}

/// The curated explanations, keyed by code (DECISION B: highest-traffic,
/// newcomer-facing codes first). Kept sorted by code; every `code` must be a
/// real [`REGISTRY`] entry and every `page` an existing Book page — both
/// enforced by `tests/diagnostics_registry.rs`. Every `example` needs its
/// compiled pair under `bynk-tests/tests/fixtures/explain/<code>/`: `bad.bynk`
/// and `good.bynk`, or, for a rule that needs a second unit or a `bynk.toml`,
/// project trees `bad/` and `good/`.
pub const EXPLANATIONS: &[Explain] = &[
    Explain {
        code: "bynk.actor.bearer_identity_not_string_constructible",
        blurb: "A `Bearer` actor's identity is minted from the token's `sub` claim, which is a \
                string, so the identity type must be one a string can become: a refined or \
                opaque `String`. Any other type has no way to be built from the claim, and an \
                actor with no `identity` at all is refused for the same reason. Declare a type \
                over `String`, such as `type UserId = String where NonEmpty`, and name it as the \
                `identity`. An `Oidc` actor has the same rule under \
                `bynk.actor.oidc_identity_not_string_constructible`.",
        example: "type Age = Int where NonNegative\n\
                  actor User { auth = Bearer(secret = \"JWT_KEY\"), identity = Age }      // ✗ a token's `sub` is a string, not an `Int`\n\
                  \n\
                  type UserId = String where NonEmpty\n\
                  actor User { auth = Bearer(secret = \"JWT_KEY\"), identity = UserId }   // ✓ minted from `sub`",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.bearer_missing_secret",
        blurb: "A `Bearer` actor checks a JWT signed with a shared key, so the compiler needs to \
                know which key. Name the environment variable that holds it: \
                `Bearer(secret = \"JWT_KEY\")`. The string is the variable's name, never the key \
                itself, so no secret is written in the source; the `Secrets` capability reads the \
                value at run time. An `Oidc` actor needs no secret, because it checks the \
                provider's public keys, and a `Signature` actor has its own \
                `bynk.actor.signature_missing_secret`.",
        example: "type UserId = String where NonEmpty\n\n\
                  actor User { auth = Bearer, identity = UserId }                         // ✗ no signing secret named\n\n\
                  actor User { auth = Bearer(secret = \"JWT_KEY\"), identity = UserId }     // ✓ the env var that holds the key",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.binder_shadows_param",
        blurb: "A `by` clause with a binder, `by who: Visitor`, adds a name to the handler body \
                beside its parameters. If the binder is spelt like a parameter it would hide it, \
                so a body that read `body` would silently get the actor instead of the request \
                body. The compiler refuses the clash rather than choose one. Rename the binder \
                or the parameter. A `by` with no binder, `by Visitor`, adds no name and cannot \
                clash.",
        example: "on POST(\"/echo\") (body: String) -> Effect[HttpResult[String]] by body: Visitor {   // ✗ `body` is already a parameter\n\n\
                  on POST(\"/echo\") (body: String) -> Effect[HttpResult[String]] by who: Visitor {   // ✓ a name of its own",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.actor.by_on_agent",
        blurb: "`by` names who is calling and has the compiler verify it at the edge of the \
                program: an HTTP request, a queue message, a cron tick. An agent handler is not \
                an edge. It is reached only from a service or another agent, which has already \
                been through its own check, so there is no caller left to verify and the clause \
                would be silently ignored. The compiler refuses it instead of letting it look \
                like protection. Remove the clause, and put the `by` on the service handler that \
                calls the agent, passing on whatever the agent needs as an argument.",
        example: "on call bump() -> Effect[Int] by v: Visitor {   // ✗ an agent handler has no actor\n\n\
                  on call bump() -> Effect[Int] {                // ✓ the service in front of it holds the `by`",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.actor.duplicate_sum_scheme",
        blurb: "Peers in a `by who: A | B` clause are told apart by their authentication scheme, so two peers of the same scheme can't both be reached: the second would never be chosen. Keep one peer per scheme, or fold the two into a single actor. A catch-all peer placed too early is a different mistake, `bynk.actor.unreachable_sum_arm`.",
        example: "type UserId = String where NonEmpty\nactor One { auth = Bearer(secret = \"KEY_A\"), identity = UserId }\nactor Two { auth = Bearer(secret = \"KEY_B\"), identity = UserId }\n\nservice api from http {\n  on POST(\"/x\") () -> Effect[HttpResult[String]] by who: One | Two {   // ✗ two `Bearer` peers\n\nservice api from http {\n  on POST(\"/x\") () -> Effect[HttpResult[String]] by who: One | Visitor {   // ✓ one peer per scheme",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.actor.foreign_context_name_unkeyable",
        blurb: "A foreign caller signs its calls under a key named from the context's name, `BYNK_CALL_TO_<CONTEXT>`. The name must therefore be lower-case letters and digits in dot-separated segments, each starting with a letter, with single underscores between runs: no `__`, and no trailing `_`. Rename the context, or move the foreign actor to one whose name qualifies.",
        example: "context shop.orders_\nactor Legacy { auth = Foreign(secret = \"LEGACY_CALL_KEY\") }   // ✗ no key can be named from `shop.orders_`\n\ncontext shop.orders\nactor Legacy { auth = Foreign(secret = \"LEGACY_CALL_KEY\") }   // ✓",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.foreign_duplicate_secret",
        blurb: "A context tells its foreign callers apart by the secret their calls are signed under, and `bynk deploy` mints one secret per name. Two actors on one secret could each pass for the other, so a foreign actor's secret may be no other actor's. Give each its own variable.",
        example: "actor Legacy { auth = Foreign(secret = \"SHARED_KEY\") }\nactor Ledger { auth = Foreign(secret = \"SHARED_KEY\") }          // ✗ `Legacy` already uses it\n\nactor Ledger { auth = Foreign(secret = \"LEDGER_CALL_KEY\") }     // ✓ its own variable",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.foreign_identity_unsupported",
        blurb: "A `Foreign` actor is its own identity: a handler tells foreign callers apart by matching on its `by` sum, not by a value carried in. So `identity` is refused, where a `Bearer` actor takes one from its token. Remove it.",
        example: "type Who = String where NonEmpty\n\nactor Legacy { auth = Foreign(secret = \"LEGACY_CALL_KEY\"), identity = Who }   // ✗ the actor is the identity\n\nactor Legacy { auth = Foreign(secret = \"LEGACY_CALL_KEY\") }                   // ✓",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.foreign_missing_secret",
        blurb: "A `Foreign` actor is a caller that is not Bynk. Its calls are signed, and the context checks each one against a secret, so the actor must name the environment variable that holds it. The string is the variable's name, never the key, and `bynk deploy` mints the key and sets it. With no name the context has nothing to check a call against. A secret that is not a string is `bynk.actor.foreign_secret_not_string`.",
        example: "actor Legacy { auth = Foreign }                                // ✗ no secret named\n\nactor Legacy { auth = Foreign(secret = \"LEGACY_CALL_KEY\") }    // ✓ the env var that holds the key",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.foreign_name_is_context_name",
        blurb: "A foreign caller sends its actor's name as its caller, and a Bynk context's name is lower case. An actor named like a context could pass for one. Start the actor's name with a capital letter.",
        example: "actor legacy { auth = Foreign(secret = \"LEGACY_CALL_KEY\") }   // ✗ reads as a context name\n\nactor Legacy { auth = Foreign(secret = \"LEGACY_CALL_KEY\") }   // ✓ capitalised",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.foreign_reserved_secret",
        blurb: "`bynk deploy` mints the keys that Bynk contexts sign their calls to each other with, and names them `BYNK_CALL_FROM_…` and `BYNK_CALL_TO_…`. A foreign caller's secret in that range could collide with one of them, so those names are kept for contexts. Pick another name, such as `LEGACY_CALL_KEY`.",
        example: "actor Legacy { auth = Foreign(secret = \"BYNK_CALL_FROM_LEGACY\") }   // ✗ a `BYNK_CALL_` key name\n\nactor Legacy { auth = Foreign(secret = \"LEGACY_CALL_KEY\") }         // ✓ any other name",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.foreign_secret_not_string",
        blurb: "A `Foreign` actor's `secret` is the name of an environment variable, so it is written as a string. A number or any other expression names no variable. Put the variable's name in quotes. A `secret` left out altogether is `bynk.actor.foreign_missing_secret`.",
        example: "actor Legacy { auth = Foreign(secret = 42) }                   // ✗ not a variable name\n\nactor Legacy { auth = Foreign(secret = \"LEGACY_CALL_KEY\") }    // ✓ a string",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.identity_not_sealed",
        blurb: "An actor's identity is a value only its context can mint, from a verified token or caller, so its type has to be one that context owns. A built-in such as `Int`, or a type from another context, could be built by anyone, so the identity would prove nothing. Declare the identity as a type in this context, such as `type PrincipalId = Int where NonNegative`, and name that.",
        example: "actor Principal { auth = Internal, identity = Int }   // ✗ `Int` is not owned by this context\n\ntype PrincipalId = Int where NonNegative\nactor Principal { auth = Internal, identity = PrincipalId }   // ✓ minted only here",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.missing_by_on_http",
        blurb: "An HTTP handler can be reached by anyone, and Bynk has no safe default for who \
                is calling, so each one must say, with a `by` clause, which actor it expects. \
                The compiler then emits the verification before the body runs. A public route \
                writes `by v: Visitor`; an authenticated one names an `actor` it declares. Set \
                it once on the header, `service api from http by Visitor`, and every handler \
                without its own `by` inherits it. A WebSocket `on open` handler needs one too. \
                The internal protocols (`call`, cron, queue) have a default.",
        example: "service api from http {\n\
                  \x20 on GET(\"/health\") () -> Effect[HttpResult[String]] {   // ✗ who may call this?\n\
                  \x20   Ok(\"up\")\n\
                  \x20 }\n\
                  }\n\
                  \n\
                  service api from http {\n\
                  \x20 on GET(\"/health\") () -> Effect[HttpResult[String]] by v: Visitor {   // ✓ anyone, said outright\n\
                  \x20   Ok(\"up\")\n\
                  \x20 }\n\
                  }",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.actor.oidc_identity_not_string_constructible",
        blurb: "An `Oidc` actor's identity is minted from the verified token's `sub` claim, which is a string, so the identity type must be a refined or opaque `String`. An actor with no `identity` is refused too. Declare a type over `String`, such as `type UserId = String where NonEmpty`, and name it. A `Bearer` actor has the same rule under `bynk.actor.bearer_identity_not_string_constructible`.",
        example: "type Age = Int where NonNegative\nactor User { auth = Oidc(issuer = \"https://issuer.example\", audience = \"my-api\", jwks = \"https://issuer.example/jwks\"), identity = Age }   // ✗ `sub` is a string, not an `Int`\n\ntype UserId = String where NonEmpty\nactor User { auth = Oidc(issuer = \"https://issuer.example\", audience = \"my-api\", jwks = \"https://issuer.example/jwks\"), identity = UserId }   // ✓ minted from `sub`",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.oidc_missing_audience",
        blurb: "A provider signs tokens for many services, and each token is issued for one. An `Oidc` actor therefore names its `audience`, the `aud` claim a token must carry, so a token issued for some other API is refused here even though its signature is valid. Add `audience = \"…\"` with the identifier your provider uses for this API. The issuer and key set have their own codes, `bynk.actor.oidc_missing_issuer` and `bynk.actor.oidc_missing_jwks`.",
        example: "type UserId = String where NonEmpty\nactor User { auth = Oidc(issuer = \"https://issuer.example\", jwks = \"https://issuer.example/jwks\"), identity = UserId }   // ✗ no audience\n\nactor User { auth = Oidc(issuer = \"https://issuer.example\", audience = \"my-api\", jwks = \"https://issuer.example/jwks\"), identity = UserId }   // ✓ the `aud` this API accepts",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.oidc_missing_issuer",
        blurb: "A valid signature shows that some key signed a token, not that your provider issued it. An `Oidc` actor therefore names its `issuer`, the `iss` claim a token must carry, and refuses any other. Add `issuer = \"…\"` with the provider's issuer URL. The other required settings have their own codes, `bynk.actor.oidc_missing_audience` and `bynk.actor.oidc_missing_jwks`.",
        example: "type UserId = String where NonEmpty\nactor User { auth = Oidc(audience = \"my-api\", jwks = \"https://issuer.example/jwks\"), identity = UserId }   // ✗ no issuer\n\nactor User { auth = Oidc(issuer = \"https://issuer.example\", audience = \"my-api\", jwks = \"https://issuer.example/jwks\"), identity = UserId }   // ✓ the `iss` a token must carry",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.oidc_missing_jwks",
        blurb: "An `Oidc` actor holds no shared secret. It checks a token's signature against the provider's public keys, a JSON Web Key Set that the verifier fetches from a URL, so the actor must say where it is. Add `jwks = \"…\"`, for example the provider's `/.well-known/jwks.json`. The `issuer` and `audience` are required as well, under `bynk.actor.oidc_missing_issuer` and `bynk.actor.oidc_missing_audience`.",
        example: "type UserId = String where NonEmpty\nactor User { auth = Oidc(issuer = \"https://issuer.example\", audience = \"my-api\"), identity = UserId }   // ✗ nowhere to fetch the public keys\n\nactor User { auth = Oidc(issuer = \"https://issuer.example\", audience = \"my-api\", jwks = \"https://issuer.example/jwks\"), identity = UserId }   // ✓ the provider's key set",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.oidc_not_in_sum",
        blurb: "A multi-actor `by` clause tries its peers in turn on a request it reads once. An `Oidc` actor must first fetch the provider's keys, which that path does not yet support, so an `Oidc` actor stands alone on its route. Give the route a single `by who: Member`, and serve any anonymous route separately.",
        example: "type UserId = String where NonEmpty\nactor Member { auth = Oidc(issuer = \"https://issuer.example\", audience = \"my-api\", jwks = \"https://issuer.example/jwks\"), identity = UserId }\n\nservice api from http {\n  on POST(\"/x\") () -> Effect[HttpResult[String]] by who: Member | Visitor {   // ✗ an `Oidc` peer in a sum\n\nservice api from http {\n  on POST(\"/x\") () -> Effect[HttpResult[String]] by who: Member {   // ✓ alone",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.actor.oidc_trust_from_is_secret",
        blurb: "`trustFrom` names a variable that holds trust roots as JSON. A variable an actor uses as its `secret` holds a signing key, and one variable cannot be both. Give `trustFrom` a variable of its own.",
        example: "type UserId = String where NonEmpty\ntype WorkloadId = String where NonEmpty\n\nactor User { auth = Bearer(secret = \"JWT_KEY\"), identity = UserId }\nactor Workload { auth = Oidc(issuer = \"https://i.test\", audience = \"a\", jwks = \"https://i.test/jwks\", trustFrom = \"JWT_KEY\"), identity = WorkloadId }   // ✗ `JWT_KEY` is a key\n\nactor Workload { auth = Oidc(issuer = \"https://i.test\", audience = \"a\", jwks = \"https://i.test/jwks\", trustFrom = \"OIDC_TRUST\"), identity = WorkloadId }   // ✓",
        page: "/book/reference/actors",
        anchor: "trust-roots-from-the-environment",
    },
    Explain {
        code: "bynk.actor.oidc_trust_from_not_string",
        blurb: "`trustFrom` names an environment variable whose JSON, when set, replaces the actor's declared trust roots at run time, as in a test or staging deployment. A variable is named by a string, so a number or other expression names nothing. Write the variable's name in quotes.",
        example: "type WorkloadId = String where NonEmpty\n\nactor Workload { auth = Oidc(issuer = \"https://i.test\", audience = \"a\", jwks = \"https://i.test/jwks\", trustFrom = 5), identity = WorkloadId }   // ✗ not a name\n\nactor Workload { auth = Oidc(issuer = \"https://i.test\", audience = \"a\", jwks = \"https://i.test/jwks\", trustFrom = \"OIDC_TRUST\"), identity = WorkloadId }   // ✓",
        page: "/book/reference/actors",
        anchor: "trust-roots-from-the-environment",
    },
    Explain {
        code: "bynk.actor.outside_context",
        blurb: "An `actor` says who may call a handler and what the compiler then knows about them. Only a context has handlers, and an actor's identity is sealed to the context that declares it, so only a context may declare one. A `commons` is shared code that other units `uses`. Move the `actor` into the context whose services take it.",
        example: "commons shop\n\nactor User { auth = None }   // ✗ a commons has no handlers\n\ncontext shop\n\nactor User { auth = None }   // ✓ declared where it is used",
        page: "/book/reference/actors",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.actor.refinement_base_unsupported",
        blurb: "`actor Admin = User where …` adds an authorisation check on a token's claims, and only a `Bearer` actor carries claims. So the base must be a declared `Bearer` actor: not `Visitor`, `Internal` or `Signature`, and not another refinement. Name a `Bearer` actor as the base, declaring it if need be.",
        example: "type UserId = String where NonEmpty\nactor User { auth = Bearer(secret = \"JWT_KEY\"), identity = UserId }\n\nactor Admin = Visitor where hasClaim(\"admin\")   // ✗ `Visitor` carries no claims\n\nactor Admin = User where hasClaim(\"admin\")   // ✓ a `Bearer` actor",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.actor.refinement_in_sum",
        blurb: "A refinement actor has its base's scheme and only narrows it, and peers of a sum are told apart by scheme, so it can't stand beside its base as another choice. Name the base as the peer and test the extra condition in its arm, or give the refined route its own `by who: Admin`.",
        example: "type UserId = String where NonEmpty\nactor User { auth = Bearer(secret = \"JWT_KEY\"), identity = UserId }\nactor Admin = User where hasClaim(\"admin\")\n\nservice api from http {\n  on POST(\"/x\") () -> Effect[HttpResult[String]] by who: User | Admin {   // ✗ `Admin` refines `User`\n\nservice api from http {\n  on POST(\"/x\") () -> Effect[HttpResult[String]] by who: Admin {   // ✓ its own route",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.actor.refinement_predicate_unsupported",
        blurb: "A refinement actor's `where` is checked against a token's claims, which are untyped JSON, so the predicate is a closed vocabulary: `hasClaim(\"name\")` and `claimEquals(\"name\", \"value\")`, composed with `&&`, `||` and `!`. Express the condition with those. Anything else, such as a call to a function of your own, is refused.",
        example: "type UserId = String where NonEmpty\nactor User { auth = Bearer(secret = \"JWT_KEY\"), identity = UserId }\n\nactor Admin = User where isAdmin(user)   // ✗ not a claim predicate\n\nactor Admin = User where hasClaim(\"admin\")   // ✓",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.actor.scheme_arg_kind",
        blurb: "A scheme's config values are strings and integers, except that a `SignedCookie` also takes a duration (`ttl = 12.hours`) and a bare name (`sameSite = Lax`). Every other scheme reads a string or an integer, so a duration or a bare name would be dropped without a word: a `Signature` `tolerance = 5.minutes` would read as no tolerance at all, and the replay window would be off. Write a `Signature` tolerance in whole seconds, and quote a string.",
        example: "actor Hook { auth = Signature(secret = \"K\", header = \"X-Sig\", timestamp = \"X-Ts\", tolerance = 5.minutes) }   // ✗ a duration\n\nactor Hook { auth = Signature(secret = \"K\", header = \"X-Sig\", timestamp = \"X-Ts\", tolerance = 300) }   // ✓ seconds",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.scheme_not_admissible",
        blurb: "Each protocol admits only the schemes that can be verified on it. HTTP takes \
                `None` (`Visitor`), `Bearer`, `Signature` and `Oidc`; a WebSocket takes `None` \
                and `Bearer`; `call` takes `Internal` and `Foreign`; cron and queue take \
                `Internal`. A `Visitor` on cron is wrong because nothing public calls a cron \
                handler: the platform does. Use the actor made for the protocol: `Scheduler` for \
                cron, `Producer` for queue, `Caller` for `on call`. `Caller` is admitted on \
                `on call` only.",
        example: "service jobs from cron {\n\
                  \x20 on schedule(\"* * * * *\") () -> Effect[Result[(), String]] by v: Visitor {   // ✗ cron takes an `Internal` actor\n\
                  \x20   Ok(())\n\
                  \x20 }\n\
                  }\n\
                  \n\
                  service jobs from cron {\n\
                  \x20 on schedule(\"* * * * *\") () -> Effect[Result[(), String]] by s: Scheduler {   // ✓ the cron actor\n\
                  \x20   Ok(())\n\
                  \x20 }\n\
                  }",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.signature_identity_unsupported",
        blurb: "A signature shows that a request is authentic, not who sent it, so a `Signature` actor has no principal to give an identity. The event is the handler's `body`. Remove `identity`. For an actor that carries a principal, see `bynk.actor.bearer_identity_not_string_constructible`.",
        example: "actor Hook { auth = Signature(secret = \"WEBHOOK_KEY\", header = \"X-Sig\"), identity = String }   // ✗ no principal\n\nactor Hook { auth = Signature(secret = \"WEBHOOK_KEY\", header = \"X-Sig\") }                       // ✓",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.signature_missing_header",
        blurb: "A `Signature` actor reads the sender's signature from a request header, and the header's name is the sender's choice, so the actor must say which one. Add `header = \"…\"` with the name of the header that carries the HMAC. Without it the compiler has no way to find the signature to check the body against.",
        example: "actor Hook { auth = Signature(secret = \"WEBHOOK_KEY\") }   // ✗ which header holds the signature?\n\nactor Hook { auth = Signature(secret = \"WEBHOOK_KEY\", header = \"X-Sig\") }   // ✓",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.signature_missing_secret",
        blurb: "A `Signature` actor checks an HMAC of the request body, so the compiler needs the key to check it against. Name the environment variable that holds it: `secret = \"WEBHOOK_KEY\"`. The string is the variable's name, never the key itself, so no secret is written in the source. A `Bearer` actor has its own `bynk.actor.bearer_missing_secret`.",
        example: "actor Hook { auth = Signature(header = \"X-Sig\") }   // ✗ no signing secret named\n\nactor Hook { auth = Signature(secret = \"WEBHOOK_KEY\", header = \"X-Sig\") }   // ✓ the env var that holds the key",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.signature_requires_body",
        blurb: "A `Signature` is an HMAC over the raw request body, so the handler must take that body: one with no `body` parameter would vouch for data it never reads. Add `(body: T)`, where `T` is the payload's type. A route that takes no payload has nothing for a signature to protect.",
        example: "actor Hook { auth = Signature(secret = \"WEBHOOK_KEY\", header = \"X-Sig\") }\n\nservice api from http {\n  on POST(\"/x\") () -> Effect[HttpResult[String]] by Hook {   // ✗ no `body` parameter\n  on POST(\"/x\") (body: String) -> Effect[HttpResult[String]] by Hook {   // ✓ the signed payload",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.signature_tolerance_without_timestamp",
        blurb: "`tolerance` is how many seconds old a signed request may be before it is refused, which limits replay of a captured one. The age is read from a timestamp header, so `tolerance` needs `timestamp = \"…\"` naming it; alone it has nothing to compare against. Add the `timestamp`, or drop `tolerance` and accept that a captured request can be replayed.",
        example: "actor Hook { auth = Signature(secret = \"WEBHOOK_KEY\", header = \"X-Sig\", tolerance = 300) }   // ✗ age of what?\n\nactor Hook { auth = Signature(secret = \"WEBHOOK_KEY\", header = \"X-Sig\", timestamp = \"X-Time\", tolerance = 300) }   // ✓",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.signed_cookie_bad_arg",
        blurb: "Each `SignedCookie` key takes one kind of value: `secret`, `previous` and `cookie` are strings, `ttl` is a duration, and `sameSite` is `Lax`, `Strict` or `Unrestricted`. Fix the value or remove the key.",
        example: "actor User { auth = SignedCookie(secret = 5, ttl = 12.hours), identity = UserId }   // ✗\n\nactor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_bad_name",
        blurb: "A cookie name is an RFC 6265 token: letters, digits and a few punctuation characters, with no space, `=`, `;`, `,` or quote. A browser would drop or misread anything else. The default, `__Host-session`, is always legal.",
        example: "actor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours, cookie = \"my session\"), identity = UserId }   // ✗\n\nactor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_bad_ttl",
        blurb: "`ttl` must be a `Duration` literal that is positive and a whole number of seconds, because it becomes both a `Max-Age` and a signed expiry in seconds. Write `12.hours` or `30.minutes`.",
        example: "actor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 1500.milliseconds), identity = UserId }   // ✗\n\nactor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_duplicate_key",
        blurb: "A `SignedCookie` config names each key once. A repeated `secret` or `ttl` would have the first value win and the second ignored without a word, in the scheme that guards a session. Keep the one you mean.",
        example: "actor User { auth = SignedCookie(secret = \"A\", secret = \"B\", ttl = 12.hours), identity = UserId }   // ✗ two secrets\n\nactor User { auth = SignedCookie(secret = \"A\", ttl = 12.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_identity_not_string_constructible",
        blurb: "The identity is minted from the cookie's `sub` claim, which is a string, so it must be a refined or opaque `String` type that this context owns. Declare `identity = T`.",
        example: "actor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours) }   // ✗\n\nactor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_issue_needs_clock",
        blurb: "`User.issue(identity)` signs a cookie that expires, so it reads the current time from the `Clock` capability, and a handler's dependencies are always visible in its `given` clause (and a test can stub the clock to know exactly the cookie it gets). Add `consumes bynk { Clock }` to the context and `given Clock` to the handler.",
        example: "let c <- User.issue(body)   // ✗ the handler lists no `given Clock`\n\nlet c <- User.issue(body)   // ✓ with `given Clock` on the handler",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_issue_outside_handler",
        blurb: "The signing key reaches a service handler, which is where a request arrives and a `Set-Cookie` is sent. A provider or an agent holds only the capabilities in its `given`, so `User.issue(identity)` there would find no key and fault on every request. Issue the cookie in the handler.",
        example: "User.issue(id)   // ✗ in a provider or an agent\n\nlet c <- User.issue(body)   // ✓ in a service handler",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_missing_secret",
        blurb: "A `SignedCookie` actor signs the session cookies it issues and verifies the ones it receives, so it needs a key. `secret` names the environment variable holding it, never the key itself. `bynk deploy` mints a random 32-byte key for it when the variable is unset, so you never choose one.",
        example: "actor User { auth = SignedCookie(ttl = 12.hours), identity = UserId }   // ✗\n\nactor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_missing_ttl",
        blurb: "A session cookie that never expires is the failure a language should not allow by omission, so `ttl` is required. It is how long an issued cookie is valid: the browser's `Max-Age` and the signed expiry both come from it. A stateless cookie cannot be revoked before it expires, so keep it short.",
        example: "actor User { auth = SignedCookie(secret = \"SESSION_KEY\"), identity = UserId }   // ✗\n\nactor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_not_in_sum",
        blurb: "A multi-actor `by` clause tries its peers in turn. A cookie is an ambient credential, so the CSRF check must apply only when the cookie is what authenticated the request, and that per-arm rule is not built yet. A `SignedCookie` actor therefore stands alone on its route: give it a single `by who: User` and serve any anonymous route separately.",
        example: "on POST(\"/x\") () -> Effect[HttpResult[String]] by who: User | Visitor {   // ✗ a `SignedCookie` peer in a sum\n\non POST(\"/x\") () -> Effect[HttpResult[String]] by who: User {   // ✓ alone",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_public_cache",
        blurb: "A route guarded by a session cookie answers one user, so a shared cache must not store its response, or it would serve one user's answer to another. Use `scope: private`, which lets only the browser keep it, or drop `@cache`.",
        example: "@cache(maxAge: 5.minutes, scope: public)   // ✗ a shared cache would replay it\n\n@cache(maxAge: 5.minutes, scope: private)   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_read_directly",
        blurb: "A `SignedCookie` actor reads its session cookie at the boundary, verifies it and hands the handler only the identity, so the signed value never reaches handler code. A `cookies` record on the same route that names that cookie (by its own name or `@name`) would ask for the raw value, so it is refused, and the router leaves the cookie out of a raw `RequestCookies` view too. Read the identity from the `by` binder, and keep other cookies (a theme, a language) in the record.",
        example: "type Prefs = {\n  @name(\"__Host-session\") session: Option[String],   // ✗ the session cookie\n  theme: Option[String],\n}\n\ntype Prefs = {\n  theme: Option[String],   // ✓ other cookies only\n}",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_secret_shared",
        blurb: "A session key signs one actor's cookies and nothing else. If two actors shared a variable, a cookie issued for one would verify for the other, and a Bearer token could be replayed as a cookie. Give each actor its own variable; `previous` is the key being rotated out, so it differs from `secret` too.",
        example: "actor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }\nactor Admin { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 1.hours), identity = UserId }   // ✗ same key\n\nactor Admin { auth = SignedCookie(secret = \"ADMIN_SESSION_KEY\", ttl = 1.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_ttl_long",
        blurb: "A stateless signed cookie cannot be revoked before it expires: a copy a user already took keeps working for the whole `ttl`. Beyond 30 days that is a long exposure, so it warns. Shorten the `ttl`, or keep the session on the server (see `examples/sessions`) if you need to log out everywhere.",
        example: "actor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 31.days), identity = UserId }   // ⚠ long-lived\n\nactor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_unknown_key",
        blurb: "A `SignedCookie` takes `secret`, `ttl`, `cookie`, `previous` and `sameSite`, and nothing else. The set is closed so a misspelt option is refused rather than ignored. The cookie's `Domain` and `Path` are not configurable on purpose: the issued cookie is always host-only and `Path=/`, which keeps a `__Host-` name valid.",
        example: "actor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours, domain = \"example.com\"), identity = UserId }   // ✗\n\nactor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }   // ✓",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_unprefixed",
        blurb: "A `__Host-` cookie name tells the browser to keep the cookie only if it is `Secure`, host-only and `Path=/`, so a sibling subdomain cannot set or overwrite it. An unprefixed session cookie is open to cookie tossing from any subdomain, which is rarely intended. The default is `__Host-session`; drop `cookie` or give it a `__Host-` name.",
        example: "actor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours, cookie = \"session\"), identity = UserId }   // ⚠ not `__Host-`\n\nactor User { auth = SignedCookie(secret = \"SESSION_KEY\", ttl = 12.hours), identity = UserId }   // ✓ `__Host-session` by default",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_write_directly",
        blurb: "A `SignedCookie` actor's session cookie is minted by `Actor.issue(identity)`, which signs it with the actor's key, and removed by `Actor.expire()`, and the CSRF check keys off that actor. A cookie record's `R.field.cookie(v)` or `R.field.expire()` whose field is read under the session cookie's name would write or clear the credential without any of that, so it is refused. Keep the session cookie out of the record and use the actor's own members.",
        example: "type Prefs = {\n  @name(\"__Host-session\") session: Option[String],\n  theme: Option[String],\n}\n\nOk(\"x\").withCookie(Prefs.session.cookie(\"forged\"))   // ✗ the session cookie\n\nOk(\"x\").withCookie(Prefs.theme.cookie(\"dark\"))        // ✓ another cookie",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.signed_cookie_write_literal",
        blurb: "A `Cookie { name: \"__Host-session\", … }` literal named like a `SignedCookie` actor's session cookie would write or clear the credential by hand, without the actor's signing key, expiry or CSRF seam, so it is refused. Write it with `Actor.issue(identity)` and remove it with `Actor.expire()`.",
        example: "Ok(\"x\").withCookie(Cookie { name: \"__Host-session\", value: \"forged\" })   // ✗ the actor's cookie\n\nOk(\"x\").withCookie(Cookie { name: \"theme\", value: \"dark\" })            // ✓ another cookie",
        page: "/book/reference/actors",
        anchor: "signed-cookies",
    },
    Explain {
        code: "bynk.actor.sum_requires_binder",
        blurb: "`by A | B` tries the actors in order and the first that verifies the caller wins. \
                Which one that was is only known when a request arrives, so the handler learns it \
                through the binder and `match`es on it. With no binder the answer would be \
                thrown away and the sum would do nothing a single actor does not. Add one, \
                `by who: A | B`, and match `who` in the body. A single actor needs no binder.",
        example: "on GET(\"/x\") () -> Effect[HttpResult[String]] by User | Visitor {       // ✗ which one was it?\n\n\
                  on GET(\"/x\") () -> Effect[HttpResult[String]] by who: User | Visitor {   // ✓ `who` is the resolved actor",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.actor.unknown_actor",
        blurb: "A `by` clause must name an actor the compiler can find: one declared with \
                `actor` in this context, or a prelude actor (`Visitor`, `Caller`, `Scheduler` or \
                `Producer`). Anything else is a misspelling or a missing declaration. The \
                compiler will not guess, because the actor decides how callers are verified. \
                Fix the spelling, or declare the actor first with `actor Name { auth = … }`.",
        example: "on GET(\"/health\") () -> Effect[HttpResult[String]] by v: Visitr {     // ✗ no actor of that name\n\n\
                  on GET(\"/health\") () -> Effect[HttpResult[String]] by v: Visitor {   // ✓ the prelude actor",
        page: "/book/reference/actors",
        anchor: "prelude-actors",
    },
    Explain {
        code: "bynk.actor.unknown_scheme",
        blurb: "An actor's `auth` names how its callers are verified, and the compiler writes \
                that check itself, so the scheme must be one it knows: `None`, `Internal`, \
                `Bearer`, `Signature`, `Oidc`, `Foreign` or `SignedCookie`. There are no user-defined schemes, \
                and a name outside the set is usually a typo or a scheme borrowed from another \
                framework. Choose the one that matches how callers prove who they are, such as \
                `Bearer` for a JWT signed with a shared key; the actors page lists what each \
                one needs.",
        example: "actor Robot { auth = Bogus }   // ✗ not a scheme the compiler knows\n\n\
                  actor Robot { auth = None }    // ✓ one of the seven",
        page: "/book/reference/actors",
        anchor: "schemes",
    },
    Explain {
        code: "bynk.actor.unreachable_sum_arm",
        blurb: "In `by who: A | B` the peers are tried in order and the first to verify wins. `Visitor`, or any other `None` peer, accepts everyone, so a peer after it can never be reached and its check would never run. Put the catch-all last. A repeated scheme is a different mistake, `bynk.actor.duplicate_sum_scheme`.",
        example: "type UserId = String where NonEmpty\nactor User { auth = Bearer(secret = \"JWT_KEY\"), identity = UserId }\n\nservice api from http {\n  on POST(\"/x\") () -> Effect[HttpResult[String]] by who: Visitor | User {   // ✗ `Visitor` accepts everyone first\n\nservice api from http {\n  on POST(\"/x\") () -> Effect[HttpResult[String]] by who: User | Visitor {   // ✓ the catch-all last",
        page: "/book/reference/actors",
        anchor: "the-by-clause",
    },
    Explain {
        code: "bynk.adapter.consumes_context",
        blurb: "Adapter dependencies run from adapter to adapter. An adapter sits at the host boundary and is wired in process beside its binding, but a context is a deployment unit with services of its own, so it cannot be consumed from there. Consume a capability that another adapter exports, or put the code that needs a context in a context.",
        example: "// logging.bynk\ncontext logging { … }\n\n// tokens.bynk\nadapter tokens {\n    consumes logging { Log }    // ✗ `logging` is a context\n}\n\n// logging.bynk\nadapter logging { … }         // ✓ an adapter, so it may be consumed",
        page: "/book/reference/adapters",
        anchor: "adapter-dependencies",
    },
    Explain {
        code: "bynk.adapter.consumes_requires_selection",
        blurb: "An adapter depends on another adapter's capabilities, never on its services, so its `consumes` must say which: `consumes logging { Log }`. The whole-unit form and `as Alias` belong to contexts. Braces that select only types are refused too, since they name no capability. List the capabilities the adapter needs.",
        example: "adapter tokens {\n    consumes logging              // ✗ which capabilities?\n}\n\nadapter tokens {\n    consumes logging { Log }      // ✓ the capabilities it needs\n}",
        page: "/book/reference/adapters",
        anchor: "adapter-dependencies",
    },
    Explain {
        code: "bynk.adapter.disallowed_item",
        blurb: "An adapter wraps foreign code, so it holds only what describes that boundary: \
                capabilities, the types that cross it, external providers and helper functions. A \
                `service`, an `agent` or an `actor` has behaviour, state and an address of its own, \
                and those belong to a context. Move the item into a context that `consumes` the \
                adapter's capability, and leave the adapter as an interface to the foreign code.",
        example: "adapter tokens {\n\
                \x20 binding \"./tokens.binding.ts\"\n\
                \x20 provides Jwt = JoseJwt\n\
                \x20 service login {                       // ✗ an adapter may not declare a service\n\
                \x20   on call(x: Int) -> Effect[Int] { … }\n\
                \x20 }\n\
                }\n\
                \n\
                adapter tokens {\n\
                \x20 binding \"./tokens.binding.ts\"\n\
                \x20 provides Jwt = JoseJwt                // ✓ capabilities and providers only\n\
                }",
        page: "/book/reference/adapters",
        anchor: "anatomy",
    },
    Explain {
        code: "bynk.adapter.duplicate_binding",
        blurb: "An adapter's `binding` names the one TypeScript module that supplies its external providers, so there is one clause. With two, the compiler would have no way to say which module a provider comes from. Merge them into one clause; the module it names can import whatever others it needs.",
        example: "adapter weather {\n    binding \"./station.binding.ts\"\n    binding \"./extra.binding.ts\"      // ✗ an adapter has one binding\n}\n\nadapter weather {\n    binding \"./station.binding.ts\"    // ✓ one clause\n}",
        page: "/book/reference/adapters",
        anchor: "anatomy",
    },
    Explain {
        code: "bynk.adapter.include_duplicate",
        blurb: "Each module is listed once: the compiler generates one loader, `./x.wasm.js`, per file. Two entries for one file add nothing, and `./a.wasm` and `a.wasm` are the same file once the path is normalised. Remove the repeat, or correct the name if a different module was meant.",
        example: "adapter math {\n    binding \"./math.binding.ts\" includes [\"./a.wasm\", \"a.wasm\"]    // ✗ the same file twice\n}\n\nadapter math {\n    binding \"./math.binding.ts\" includes [\"./a.wasm\"]              // ✓ listed once\n}",
        page: "/book/reference/adapters",
        anchor: "anatomy",
    },
    Explain {
        code: "bynk.adapter.include_kind",
        blurb: "`includes` is for WebAssembly modules. The compiler generates a loader for each, `./x.wasm.js`, that gives the binding a compiled module, and it picks them by the `.wasm` extension. Any other file has no such loader. Name a `.wasm` module.",
        example: "adapter math {\n    binding \"./math.binding.ts\" includes [\"./data.bin\"]    // ✗ not a `.wasm` file\n}\n\nadapter math {\n    binding \"./math.binding.ts\" includes [\"./adder.wasm\"]   // ✓ a WebAssembly module\n}",
        page: "/book/reference/adapters",
        anchor: "anatomy",
    },
    Explain {
        code: "bynk.adapter.include_missing",
        blurb: "`includes` ships WebAssembly modules beside the binding, so each file it names has to exist: the path is read relative to the adapter's source file. A misspelt name, or a module left out of the tree, is the usual cause. Correct the path, or add the file.",
        example: "adapter math {\n    binding \"./math.binding.ts\" includes [\"./absent.wasm\"]    // ✗ no such file beside the adapter\n}\n\nadapter math {\n    binding \"./math.binding.ts\" includes [\"./adder.wasm\"]     // ✓ the module exists\n}",
        page: "/book/reference/adapters",
        anchor: "anatomy",
    },
    Explain {
        code: "bynk.adapter.include_outside",
        blurb: "An included module is part of the adapter's own tree, and, for a dependency, of the tree the lock checksums. So each path is relative to the adapter's file and stays inside its directory: no absolute path, and no `..`. Move the module into the adapter's directory, or a subdirectory of it, and name it from there.",
        example: "adapter math {\n    binding \"./math.binding.ts\" includes [\"../shared.wasm\"]    // ✗ leaves the adapter's directory\n}\n\nadapter math {\n    binding \"./math.binding.ts\" includes [\"./adder.wasm\"]       // ✓ inside it\n}",
        page: "/book/reference/adapters",
        anchor: "anatomy",
    },
    Explain {
        code: "bynk.adapter.no_binding",
        blurb: "An external provider, `provides Jwt = JoseJwt` with no body, is written in \
                TypeScript, not Bynk. The adapter's `binding` clause names that TypeScript module, \
                resolved relative to the adapter's source file, and the compiler reads it to wire \
                the provider in. The code fires when the clause is missing, and when the named file \
                cannot be read. Add `binding \"./tokens.binding.ts\"` and create the file, exporting \
                a class named for the provider.",
        example: "adapter tokens {                        // ✗ an external provider, but no binding clause\n\
                \x20 provides Jwt = JoseJwt\n\
                }\n\
                \n\
                adapter tokens {\n\
                \x20 binding \"./tokens.binding.ts\"         // ✓ names the module that supplies JoseJwt\n\
                \x20 provides Jwt = JoseJwt\n\
                }",
        page: "/book/reference/adapters",
        anchor: "anatomy",
    },
    Explain {
        code: "bynk.adapter.provider_has_body",
        blurb: "An adapter wraps code written in TypeScript. Its provider has no body: `provides Jwt = JoseJwt`, and the binding exports a class of that name. A Bynk body would be a second implementation beside the binding's. Remove the body and implement the operations in the binding, or, for a provider written in Bynk, move it to a context. An external provider in a context is `bynk.context.external_provider`.",
        example: "adapter tokens {\n    binding \"./tokens.binding.ts\"\n\n    capability Jwt {\n        fn sign(secret: String) -> Effect[String]\n    }\n\n    provides Jwt = JoseJwt {          // ✗ the binding supplies it\n        fn sign(secret: String) -> Effect[String] { secret }\n    }\n}\n\nadapter tokens {\n    binding \"./tokens.binding.ts\"\n\n    capability Jwt {\n        fn sign(secret: String) -> Effect[String]\n    }\n\n    provides Jwt = JoseJwt            // ✓ no body\n}",
        page: "/book/reference/adapters",
        anchor: "anatomy",
    },
    Explain {
        code: "bynk.agent.construction_arity",
        blurb: "An agent is addressed by its key, so constructing one takes exactly one \
                argument: the value of its single `key` field. `Counter(\"a\")` names the \
                instance with key `\"a\"`. Two arguments or none do not say which instance is \
                meant. Pass the one key. A key of the wrong type is `bynk.agent.key_mismatch`.",
        example: "agent Counter {\n\
                  \x20 key id: String\n\
                  \x20 store n: Cell[Int]\n\
                  \n\
                  \x20 on call get() -> Effect[Int] {\n\
                  \x20   n\n\
                  \x20 }\n\
                  }\n\
                  \n\
                  let v <- Counter(\"a\", \"b\").get()   // ✗ one key, two arguments\n\
                  let v <- Counter(\"a\").get()         // ✓ one key",
        page: "/book/reference/agents",
        anchor: "addressing-and-calling",
    },
    Explain {
        code: "bynk.agent.handler_arity",
        blurb: "A handler call is checked against the handler's declaration, as a function \
                call is: it must pass one argument for each parameter, no more and no fewer. \
                The agent's key is not one of them, since it was given when the agent was \
                constructed. Match the call to the parameter list. If the handler is missing a \
                parameter it needs, add it to the declaration and update every caller.",
        example: "agent Counter {\n\
                  \x20 key id: String\n\
                  \x20 store n: Cell[Int]\n\
                  \n\
                  \x20 on call get() -> Effect[Int] {\n\
                  \x20   n\n\
                  \x20 }\n\
                  }\n\
                  \n\
                  let v <- Counter(\"a\").get(1)   // ✗ `get` takes no arguments\n\
                  let v <- Counter(\"a\").get()    // ✓",
        page: "/book/reference/agents",
        anchor: "addressing-and-calling",
    },
    Explain {
        code: "bynk.agent.handler_not_found",
        blurb: "An agent is used through the handlers it declares with `on call`, so a method \
                name it does not declare is a mistake: usually a misspelling, or a handler that \
                was never written. Check the name against the agent's declaration, or add the \
                handler. To read a `store` field from a caller, add a handler that returns it.",
        example: "agent Counter {\n\
                  \x20 key id: String\n\
                  \x20 store n: Cell[Int]\n\
                  \n\
                  \x20 on call get() -> Effect[Int] {\n\
                  \x20   n\n\
                  \x20 }\n\
                  }\n\
                  \n\
                  let v <- Counter(\"a\").nope()   // ✗ `Counter` declares no `nope`\n\
                  let v <- Counter(\"a\").get()    // ✓ a declared handler",
        page: "/book/reference/agents",
        anchor: "addressing-and-calling",
    },
    Explain {
        code: "bynk.agent.key_mismatch",
        blurb: "The key is what picks an instance, so its type is fixed by the agent: the type \
                of its `key` field. An agent keyed by `String` cannot be addressed by an `Int`, \
                because `1` and `\"1\"` would otherwise be easy to confuse and could name \
                different instances. Pass a value of the key's type. The number of key \
                arguments is `bynk.agent.construction_arity`.",
        example: "agent Counter {\n\
                  \x20 key id: String\n\
                  \x20 store n: Cell[Int]\n\
                  \n\
                  \x20 on call get() -> Effect[Int] {\n\
                  \x20   n\n\
                  \x20 }\n\
                  }\n\
                  \n\
                  let v <- Counter(1).get()     // ✗ the key is a `String`\n\
                  let v <- Counter(\"a\").get()   // ✓",
        page: "/book/reference/agents",
        anchor: "addressing-and-calling",
    },
    Explain {
        code: "bynk.agent.migrate_duplicate",
        blurb: "A stored record carries the schema version it was written at, and is loaded through the one `migrate from N` step for that version. Two steps for the same `N` would leave the compiler to choose between them, so the second is refused. Merge them into one step that assigns every field the move from version `N` needs. A step that assigns one field twice is `bynk.agent.migrate_duplicate_assignment`.",
        example: "type CartV1 = { owner: String, total: Int }\n\nmigrate from 1 (old: CartV1) {\n    ownerName := old.owner\n}\nmigrate from 1 (old: CartV1) {    // ✗ version 1 again\n    totalCents := old.total * 100\n}\n\nmigrate from 1 (old: CartV1) {\n    ownerName := old.owner\n    totalCents := old.total * 100    // ✓ one step does both\n}",
        page: "/book/reference/agents",
        anchor: "migrations",
    },
    Explain {
        code: "bynk.agent.migrate_duplicate_assignment",
        blurb: "A migration step builds the new state like a record literal: each `store` field takes at most one value. A bare field name in the body reads the value carried over, never an earlier assignment, so a second `:=` to a field cannot build on the first and is refused. Combine the two into one expression of `old` and the carried fields. Two steps for one version is `bynk.agent.migrate_duplicate`.",
        example: "type CartV1 = { owner: String, total: Int }\n\nmigrate from 1 (old: CartV1) {\n    totalCents := old.total\n    totalCents := old.total * 100    // ✗ `totalCents` again\n}\n\nmigrate from 1 (old: CartV1) {\n    totalCents := old.total * 100    // ✓ one value\n}",
        page: "/book/reference/agents",
        anchor: "migrations",
    },
    Explain {
        code: "bynk.agent.migrate_impure",
        blurb: "A migration step must be pure and must not read the agent's key. A migration sweep wakes each stored instance by its object ID, which does not recover the key, so a step cannot depend on who the instance is, and it cannot use `?`, `expect` or `Effect.pure`. Compute the new value from `old` and the carried fields. If it needs the key, do that in a handler after the migration.",
        example: "type CartV1 = { owner: String, total: Int }\n\nmigrate from 1 (old: CartV1) {\n    ownerName := id    // ✗ reads the agent's key\n}\n\nmigrate from 1 (old: CartV1) {\n    ownerName := old.owner    // ✓\n}",
        page: "/book/reference/agents",
        anchor: "migrations",
    },
    Explain {
        code: "bynk.agent.outside_context",
        blurb: "An agent holds state, and a context is the unit that owns it. A `commons` is \
                code shared between contexts, which each `uses` it, so it is not given an agent, \
                a service, an actor or a capability: those are only allowed in a `context`. \
                Declare the agent in a context. A type the agent and other contexts both need \
                can stay in the commons.",
        example: "commons shop\n\
                  \n\
                  agent Counter { … }   // ✗ a commons has no state\n\
                  \n\
                  context shop\n\
                  \n\
                  agent Counter { … }   // ✓ a context owns it",
        page: "/book/reference/agents",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.agent.removed_live",
        blurb: "`removed agent Cart` tells the next deploy to delete the class `Cart` and all of its stored data. While `Cart` is still declared, that would delete the data of a class the code still uses, so the two together are refused. Delete the `agent Cart` declaration; the data goes at the next deploy, once you confirm it by name. If the class should stay, delete the `removed agent` line instead.",
        example: "removed agent Cart    // ✗ `Cart` is still declared\nagent Cart { … }\n\nremoved agent Cart\nagent Basket { … }    // ✓ `Cart` is gone",
        page: "/book/reference/agents",
        anchor: "renaming-and-removing-an-agent",
    },
    Explain {
        code: "bynk.agent.replaces_live",
        blurb: "`agent New replaces Old` renames a class: `Old`'s stored data moves to `New`. While `Old` is still declared, both would claim that data, so it is refused. To rename, delete the `Old` declaration. If the two are separate classes, drop `replaces`. Each old name has one fate, as `bynk.agent.class_conflict` says.",
        example: "agent Cart { … }\nagent Basket replaces Cart { … }    // ✗ `Cart` is still declared\nagent Basket { … }                  // ✓ a separate class; delete `Cart` to rename it",
        page: "/book/reference/agents",
        anchor: "renaming-and-removing-an-agent",
    },
    Explain {
        code: "bynk.agent.return_not_effect",
        blurb: "An agent handler reads and writes the agent's stored state, and may call other \
                agents and capabilities, so it does work in the world, not just compute a \
                value. Bynk marks that in the type: a handler must return `Effect[T]`, and its \
                caller takes the result with `<-`. A bare `Int` would claim the handler is pure. \
                Wrap the return type in `Effect`. A plain `fn`, which really is pure, returns an \
                ordinary value.",
        example: "on call get() -> Int {           // ✗ a handler that reads state is an effect\n\
                  \x20 n\n\
                  }\n\
                  \n\
                  on call get() -> Effect[Int] {   // ✓\n\
                  \x20 n\n\
                  }",
        page: "/book/reference/agents",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.agent.state_name_conflict",
        blurb: "Inside an agent's handlers and invariants, `<Agent>State` names the record of the \
                agent's `Cell` fields: for agent `Counter`, `CounterState`. A type you declare \
                with that name would be silently replaced there, so the compiler refuses it \
                instead of renaming anything: unlike the names it derives elsewhere, this one \
                can be written in a program. Rename your type.",
        example: "type CounterState = { label: String }   // ✗ the name of `Counter`'s state record\n\n\
                  type CounterLabel = { label: String }   // ✓ any other name\n\n\
                  agent Counter {\n\
                  \x20 key id: String\n\
                  \x20 store n: Cell[Int]\n\
                  \n\
                  \x20 on call add(k: Int) -> Effect[Int] { k }\n\
                  }",
        page: "/book/reference/agents",
        anchor: "reading-and-writing-state",
    },
    Explain {
        code: "bynk.agents.bad_state_initialiser",
        blurb: "A key's state is initialised automatically the first time it is used, before any handler runs, so a `store` field's `= value` must be fixed when the program compiles: a literal, a variant, `Some`/`None`/`Ok`/`Err`, a record, or `T.unsafe(lit)` for an opaque type. It cannot read `self`, a parameter or a capability, or call anything. The code is also reported for a static value of the wrong type. Write the starting value out, and set the real one from a handler. A field with no initialiser and no zero is `bynk.agents.non_zeroable_state_field`.",
        example: "store label: Cell[String] = self.id   // ✗ `self` is not available to an initialiser\n\nstore label: Cell[String] = \"\"   // ✓ a static starting value",
        page: "/book/reference/agents",
        anchor: "state-initialisation",
    },
    Explain {
        code: "bynk.agents.non_zeroable_state_field",
        blurb: "A key that has never been used is initialised automatically the first time it is \
                touched, so every `store Cell[T]` field needs a starting value. With no \
                initialiser, the compiler supplies a zero (`0`, `false`, `\"\"`, `None`, or a \
                record of zeroed fields), but only where that zero is a valid value of the type. \
                An opaque type, a sum other than `Option`, and a refined type that rules its zero \
                out have none, and the compiler will not invent a value that the type's own \
                constructors might refuse. Say what \"not set yet\" means: wrap the type in \
                `Option`, where `None` is the zero, or give the field an initialiser, \
                `store name: Cell[T] = value`.",
        example: "type Secret = opaque String\n\n\
                  store secret: Cell[Secret]                // ✗ no zero value for an opaque type\n\n\
                  store secret: Cell[Option[Secret]]        // ✓ `None` until it is set",
        page: "/book/reference/agents",
        anchor: "state-initialisation",
    },
    Explain {
        code: "bynk.asset.duplicate",
        blurb: "An asset is a name for a file, read as a value (`style.url`), so a unit declares each \
                name once: a second `asset style` would leave `style` meaning two files. Give each \
                asset its own name, or drop the repeat when it names the same file. An asset whose \
                name is a function's or a type's is `bynk.resolve.name_conflict` instead.",
        example: "asset style = \"assets/board.css\"\nasset style = \"assets/print.css\"    // ✗ `style` is already declared\n\nasset style = \"assets/board.css\"\nasset print = \"assets/print.css\"    // ✓ one name each",
        page: "/book/reference/assets",
        anchor: "declaring-an-asset",
    },
    Explain {
        code: "bynk.asset.invalid_at_path",
        blurb: "`at` serves an asset at a fixed URL, for clients that ask for a file by name, such as \
                `/favicon.ico`, so the path must be the URL of one file: it starts with `/`, does not \
                end in `/`, and uses only letters, digits and `._~-` between its slashes. It may \
                not be under `/_assets/` or `/_bynk/`, which Bynk uses itself, nor be `/_headers`, \
                `/_redirects` or `/.assetsignore`, which the platform reads as configuration. \
                Write the path of a single file.",
        example: "asset favicon = \"assets/favicon.ico\" at \"/icons/\"          // ✗ a folder, not a file\n\nasset favicon = \"assets/favicon.ico\" at \"/favicon.ico\"    // ✓",
        page: "/book/reference/assets",
        anchor: "fixed-paths",
    },
    Explain {
        code: "bynk.asset.missing_file",
        blurb: "An `asset` is a file the build ships, and the compiler reads it to hash its contents \
                into the asset's URL, so the file must exist when the project is built. The path is \
                relative to the project root and must name a regular file; a symbolic link is not \
                followed. Create the file under `assets/`, or correct the path. A file that exists \
                but cannot be read is `bynk.asset.unreadable_file`.",
        example: "asset style = \"assets/board.css\"        // ✗ no such file in the project\n\
                asset style = \"assets/board.css\"        // ✓ once assets/board.css exists",
        page: "/book/reference/assets",
        anchor: "declaring-an-asset",
    },
    Explain {
        code: "bynk.asset.outside_assets_dir",
        blurb: "An asset is served to anyone who asks, without the Worker running, so it may come \
                only from the project's `assets/` folder. If any path were allowed, a declaration \
                could publish `.env` or a source file just by naming it. Move or copy the file into \
                `assets/` and name it there. A path that is absolute or contains `..` is \
                `bynk.asset.unsafe_path` instead.",
        example: "asset style = \"styles/board.css\"        // ✗ not in the assets/ folder\n\
                asset style = \"assets/board.css\"        // ✓",
        page: "/book/reference/assets",
        anchor: "declaring-an-asset",
    },
    Explain {
        code: "bynk.asset.outside_context_or_commons",
        blurb: "An `asset` is a file the platform serves, shipped with each context that declares it or `uses` a commons that does. An adapter is the host boundary, not a deployed unit of its own, so an `asset` there is refused. Declare it in a context or a commons. For text compiled into the program rather than served, see `bynk.embed.outside_context_or_commons`.",
        example: "// app/host.bynk\nadapter app.host\n\nbinding \"./host.binding.ts\"\n\nasset style = \"assets/board.css\"        // ✗ an adapter cannot declare an asset\n\n// app/page.bynk\ncommons app.page\n\nasset style = \"assets/board.css\"        // ✓ in a commons",
        page: "/book/reference/assets",
        anchor: "declaring-an-asset",
    },
    Explain {
        code: "bynk.asset.path_conflict",
        blurb: "A fixed asset path names one file, and a Worker serves one file at it, so two assets \
                cannot both claim `/legal.txt`. One path cannot be a folder of another either, as \
                `/logo` and `/logo/icon.png` would be: a name cannot be both a file and a \
                directory. Give each asset its own path. Two assets for the same file at the same \
                path are no conflict. A route at an asset's path is `bynk.asset.shadows_route`.",
        example: "asset terms = \"assets/a.txt\" at \"/legal.txt\"\nasset privacy = \"assets/b.txt\" at \"/legal.txt\"    // ✗ a second file at `/legal.txt`\n\nasset terms = \"assets/a.txt\" at \"/terms.txt\"\nasset privacy = \"assets/b.txt\" at \"/privacy.txt\"    // ✓",
        page: "/book/reference/assets",
        anchor: "fixed-paths",
    },
    Explain {
        code: "bynk.asset.shadows_route",
        blurb: "The platform answers a fixed asset's path before the Worker runs, and it does so for \
                every method, so an HTTP handler at exactly that path can never be reached. The \
                compiler refuses it rather than ship a route that never runs. Move the route, or \
                give the asset another path. A route with a parameter that matches the asset's \
                path, such as `/:page`, still serves every other path, and is only the warning \
                `bynk.asset.may_shadow_route`.",
        example: "asset favicon = \"assets/favicon.ico\" at \"/favicon.ico\"\n\nservice web from http {\n    on GET(\"/favicon.ico\") () -> Effect[HttpResult[String]] by Visitor {    // ✗ the asset answers first\n        Ok(\"never reached\")\n    }\n}\n\nservice web from http {\n    on GET(\"/health\") () -> Effect[HttpResult[String]] by Visitor {    // ✓\n        Ok(\"ok\")\n    }\n}",
        page: "/book/reference/assets",
        anchor: "fixed-paths",
    },
    Explain {
        code: "bynk.asset.unsafe_path",
        blurb: "An asset's path must stay inside the project: it may not be empty or absolute, and it \
                may not contain a `..` segment. Assets are public, so a path that starts at the filesystem \
                root or climbs out of the project could publish a file that was never meant to ship. \
                Write the path relative to the project root, inside `assets/`. A relative path that \
                is only outside `assets/` is `bynk.asset.outside_assets_dir`.",
        example: "asset style = \"assets/../board.css\"     // ✗ a `..` segment\n\
                asset style = \"assets/board.css\"        // ✓",
        page: "/book/reference/assets",
        anchor: "declaring-an-asset",
    },
    Explain {
        code: "bynk.boundary.ambiguous_type",
        blurb: "A context can export a record whose field has a type it does not export. Reading that field through a value you got from it needs the field's type, and if two contexts you consume each declare a type of that name with different shapes, the compiler cannot tell which is meant. Rename one of the two so a name stands for one shape across the contexts you consume.",
        example: "// shop/quotes.bynk\ntype Amount = { cents: Int }\ntype Wrap = { inner: Amount }\n\n// shop/other.bynk\ntype Amount = { dollars: Int }\n\n// shop/till.bynk\nconsumes shop.quotes\nconsumes shop.other\n\n    let w <- shop.quotes.wrap()\n    Effect.pure(w.inner.cents)           // ✗ which `Amount` is `inner`?\n\n// shop/other.bynk\ntype Price = { dollars: Int }            // ✓ one name, one shape",
        page: "/book/guides/program-structure/consume-services",
        anchor: "rename-an-exported-type-that-clashes-with-one-of-yours",
    },
    Explain {
        code: "bynk.boundary.structural_mismatch",
        blurb: "A value that crosses a context boundary, as an argument to a consumed context's \
                service or as its result, is matched by shape, not by name. So where both \
                contexts declare a `Box`, the two must have the same shape, or the value is not \
                a `Box` on the other side. Give them the same fields, or rename one so the names \
                stop meeting. A type both sides need can live in a `commons` that each `uses`.",
        example: "// shop.bynk\n\
                  context shop\n\
                  \n\
                  type Box = { weight: Int }\n\
                  \n\
                  service weigh {\n    on call(b: Box) -> Effect[Int] { … }\n\
                  }\n\
                  \n\
                  // depot.bynk\n\
                  context depot\n\
                  \n\
                  consumes shop\n\
                  \n\
                  type Box = { label: String }\n\
                  \n\
                  shop.weigh(Box { label: \"a\" })        // ✗ a `depot` `Box` is not shaped like `shop`'s\n\
                  \n\
                  type Box = { weight: Int }\n\
                  \n\
                  shop.weigh(Box { weight: 3 })         // ✓ the same shape on both sides",
        page: "/book/reference/refined-types",
        anchor: "across-a-context-boundary",
    },
    Explain {
        code: "bynk.capability.base_not_instantiable",
        blurb: "A derived capability is another instance of a platform resource, with a namespace, bucket or limiter of its own. Only a platform resource kind, such as `Kv` or `R2`, has instances. A capability such as `Clock` has no resource to separate, so there is nothing to derive; use it bare, with `given Clock`.",
        example: "consumes bynk { Clock }\n\ncapability Ticks = Clock   // ✗ `Clock` has no named instances\n\nservice probe {\n    on call() -> Effect[Instant] given Clock {   // ✓ use it bare\n        Clock.now()\n    }\n}",
        page: "/book/reference/capabilities",
        anchor: "derived-capabilities",
    },
    Explain {
        code: "bynk.capability.derived_outside_context",
        blurb: "A derived capability is an instance of a platform resource, and the resource's data belongs to a context. An adapter only describes a boundary to foreign code, so it has no data to own. Declare the instance in a context that selects the base with `consumes`.",
        example: "adapter cache {\n    binding \"./cache.binding.ts\"\n    consumes bynk.cloudflare { Kv }\n    capability Sessions = Kv    // ✗ an adapter cannot own an instance\n}\n\ncontext cache {\n    consumes bynk.cloudflare { Kv }\n    capability Sessions = Kv    // ✓ a context owns its data\n}",
        page: "/book/reference/capabilities",
        anchor: "derived-capabilities",
    },
    Explain {
        code: "bynk.capability.instance_only",
        blurb: "A few platform capabilities (`Service`, `R2`, `RateLimit`, `Queue`, `Analytics`, \
                `D1` and `EdgeCache`) are never used as the bare kind. Each stands for a \
                particular resource of its own: a database, a bucket, a limiter with a limit \
                and a window, a Worker. So you declare a named instance, \
                `capability Orders = D1`, which gets a binding and a double of its own, and a \
                handler asks for that instance in `given` instead of the kind. Two instances of \
                one kind never share data. Some kinds need options on the instance, such as \
                `RateLimit @limit(20) @period(60.seconds)`.",
        example: "on call() -> Effect[()] given D1 {            // ✗ the bare kind names no database\n\n\
                  capability Orders = D1\n\
                  on call() -> Effect[()] given Orders {        // ✓ a named instance",
        page: "/book/reference/capabilities",
        anchor: "derived-capabilities",
    },
    Explain {
        code: "bynk.capability.invalid_option",
        blurb: "A derived capability, `capability Name = Kind`, can take options after the kind, each \
                one positional literal that the compiler reads while it checks the program: \
                `Service` takes `@worker(\"name\")`, `R2` an optional `@jurisdiction(\"eu\")`, \
                `RateLimit` a `@limit(n)` and a `@period(d)`, and `Queue` a `@name(\"queue\")`. \
                The value must be a literal of the option's kind, and one the platform accepts: \
                `@limit` a positive `Int`, `@period` `10.seconds` or `60.seconds`, \
                `@jurisdiction` `\"eu\"` or `\"fedramp\"`, `@tls` (on a `MySql`) `\"required\"`, \
                `\"verify-ca\"` or `\"verify-identity\"`, `@worker` a Worker name and `@name` a \
                queue name, each of 1 to 63 lowercase letters, digits and `-` (a Worker name \
                may also hold `_`). Each option is given once, and an `@worker` must name a Worker \
                outside the project, since a context of your own is reached with `consumes`. A bad \
                value would otherwise surface only when the platform refuses the deployment, so \
                it is refused here. Use one of the values the note lists. `RateLimit` is \
                experimental, so the project opts in under `[unstable]` in `bynk.toml`.",
        example: "// bynk.toml\n\
                  [unstable]\n\
                  allow = [\"bynk.cloudflare.RateLimit\"]\n\n\
                  // login.bynk\n\
                  context login\n\n\
                  consumes bynk.cloudflare { RateLimit }\n\n\
                  capability LoginLimit = RateLimit @limit(20) @period(30.seconds)    // ✗ a window is 10 or 60 seconds\n\
                  capability LoginLimit = RateLimit @limit(20) @period(60.seconds)    // ✓",
        page: "/book/reference/capabilities",
        anchor: "derived-capabilities",
    },
    Explain {
        code: "bynk.capability.missing_option",
        blurb: "A derived capability, `capability Name = Kind`, takes its configuration from \
                options after the kind, and some kinds cannot work without them: a `Service` \
                needs the Worker it reaches, `@worker(…)`, a `Queue` its `@name(…)`, a \
                `RateLimit` its `@limit(…)` and `@period(…)`, and a `StoredSecret` its \
                `@store(…)` and `@secret(…)`. There is no default. Write the options the note \
                shows. A value the platform would refuse is `bynk.capability.invalid_option`; an \
                option the kind does not take is `bynk.capability.unknown_option`.",
        example: "// bynk.toml\n\
                  [unstable]\n\
                  allow = [\"bynk.cloudflare.Queue\"]\n\
                  \n\
                  // orders.bynk\n\
                  context orders\n\
                  \n\
                  consumes bynk.cloudflare { Queue }\n\
                  \n\
                  capability OrderEvents = Queue                        // ✗ a queue must be named\n\
                  capability OrderEvents = Queue @name(\"order-events\")  // ✓",
        page: "/book/reference/capabilities",
        anchor: "derived-capabilities",
    },
    Explain {
        code: "bynk.capability.op_arity",
        blurb: "A call to a capability operation passes exactly the parameters its declaration \
                lists, so `Clock.now(1)` against `fn now() -> Effect[Int]` has one argument too \
                many. Pass the declared number of arguments. If the operation really needs another \
                parameter, add it to the capability's declaration and to every provider of it.",
        example: "capability Clock {\n\
                \x20 fn now() -> Effect[Int]\n\
                }\n\
                \n\
                Clock.now(1)                            // ✗ `now` takes no arguments\n\
                Clock.now()                             // ✓",
        page: "/book/reference/capabilities",
        anchor: "declaring-a-capability",
    },
    Explain {
        code: "bynk.capability.outside_context",
        blurb: "A capability is a typed interface to the outside world: a context declares it, \
                provides it and asks for it in `given`, and the compiler wires the provider to the \
                services that need it. A commons holds only pure declarations, so it cannot declare \
                one. Move the capability into the context that uses it, or, for one several contexts \
                share, into a context that exports it.",
        example: "commons demo\n\
                \n\
                capability Logger {                     // ✗ a commons cannot declare a capability\n\
                \x20 fn log(message: String) -> Effect[()]\n\
                }\n\
                \n\
                context demo\n\
                \n\
                capability Logger {                     // ✓ a context can\n\
                \x20 fn log(message: String) -> Effect[()]\n\
                }",
        page: "/book/reference/capabilities",
        anchor: "declaring-a-capability",
    },
    Explain {
        code: "bynk.capability.unknown_base",
        blurb: "`capability Sessions = Kv` makes a separate instance of a platform capability, so the base has to be one the context has selected: `consumes bynk.cloudflare { Kv }`. A name that is not selected is a misspelling, or a `consumes` that was left out. Correct the name, or select the capability.",
        example: "consumes bynk.cloudflare { Kv }\n\ncapability Sessions = Kvv   // ✗ no capability `Kvv` is selected\n\ncapability Sessions = Kv    // ✓ the one that is",
        page: "/book/reference/capabilities",
        anchor: "derived-capabilities",
    },
    Explain {
        code: "bynk.capability.unknown_operation",
        blurb: "A capability is an interface: its operations are exactly the `fn` signatures in its \
                declaration, and a provider supplies nothing more. Calling `Clock.nope()` asks for \
                an operation the capability does not declare, which usually means a misspelt name. \
                Call one of the declared operations, or add the new operation to the capability and \
                to each provider of it.",
        example: "capability Clock {\n\
                \x20 fn now() -> Effect[Int]\n\
                }\n\
                \n\
                Clock.nope()                            // ✗ `Clock` declares no `nope`\n\
                Clock.now()                             // ✓",
        page: "/book/reference/capabilities",
        anchor: "declaring-a-capability",
    },
    Explain {
        code: "bynk.capability.unknown_option",
        blurb: "An option after the kind in `capability Name = Kind @option(…)` must be one that \
                kind takes: `Service` takes `@worker`, `R2` `@jurisdiction`, `RateLimit` \
                `@limit` and `@period`, `Queue` `@name`, `MySql` `@tls`, and `StoredSecret` `@store` and \
                `@secret`, while `Kv`, `Analytics`, `D1`, `EdgeCache` and `Postgres` take none. One the kind \
                does not take is refused rather than ignored, since it is probably a misspelling \
                or the wrong kind. Drop it, or use one of the options the note lists. An option \
                that is known but has a bad value is `bynk.capability.invalid_option`.",
        example: "capability Sessions = Kv @ttl          // ✗ `Kv` instances take no options\n\
                  capability Sessions = Kv               // ✓",
        page: "/book/reference/capabilities",
        anchor: "derived-capabilities",
    },
    Explain {
        code: "bynk.cell.invalid_target",
        blurb: "A `:=` replaces the value of a `Cell`, the one store kind that holds a single value. A `Set`, `Map`, `Log` or `Cache` has no one value to replace; each changes through its own operations (`add`, `put`, `append`), and a name that is not a `store` field of the agent has nothing to write. Use the operation the field's kind offers, or declare the field `Cell[T]` if a single replaceable value is what you want. The same check covers a `migrate from` step that assigns a field.",
        example: "store ids: Set[Int]\n\nids := n               // ✗ a `Set` has no single value to replace\n\nlet _ <- ids.add(n)    // ✓ the operation the kind offers",
        page: "/book/reference/agents",
        anchor: "reading-and-writing-state",
    },
    Explain {
        code: "bynk.cell.self_reference",
        blurb: "A `:=` writes a `Cell` store field, and its right-hand side may not read the cell \
                it is writing. Computing the new value from the old one is a hidden \
                read-modify-write; Bynk asks for `.update(fn)` instead, so the \
                dependency on the prior value is written down where a reader (and a \
                retry) can see it. A `:=` is for a value that does not depend on what \
                the cell held.",
        example: "count := count + 1                       // ✗ reads `count` while writing it\n\n\
                  let _ <- count.update((c) => c + 1)      // ✓ awaited, as an effect; the combiner gets the old value",
        page: "/book/reference/agents",
        anchor: "reading-and-writing-state",
    },
    Explain {
        code: "bynk.cloudflare.edge_cache_duplicate",
        blurb: "Workers Cache is one setting for the whole Worker, and a context has one Worker, so a context declares at most one `EdgeCache` instance, which both turns the cache on and purges it. A second would be another handle on the same cache, so it is refused. Delete it and use the first instance wherever you need it, with `given`.",
        example: "capability Edge = EdgeCache\ncapability Cdn = EdgeCache    // ✗ one `EdgeCache` per context\n\ncapability Edge = EdgeCache   // ✓ use `Edge` everywhere",
        page: "/book/guides/effects-and-capabilities/cache-at-the-edge",
        anchor: "turn-the-cache-on",
    },
    Explain {
        code: "bynk.cloudflare.edge_cache_exported",
        blurb: "A capability you `export` is built by each context that consumes it, in that context's own Worker. A provider that reaches `EdgeCache` would purge from there, and a purge reaches only the cache of the Worker it runs in, so this context's cache would keep the stale response. Keep the capability inside the context and have a service route do the purge, which another context can reach by calling that service.",
        example: "exports capability { Takedown }\n\ncapability Edge = EdgeCache\n\ncapability Takedown {   // ✗ exported, but its provider purges through `Edge`\n    fn take(path: String) -> Effect[Bool]\n}\n\nprovides Takedown = Purging given Edge {\n    fn take(path: String) -> Effect[Bool] {\n        Edge.purge([path])\n    }\n}\n\nservice operator from http {\n    on POST(\"/purge/:hex\") (hex: String) -> Effect[HttpResult[Bool]] by v: Visitor given Edge {   // ✓ purge from a service handler\n        let purged <- Edge.purge([\"/dl/\\(hex)\"])\n        Ok(purged)\n    }\n}",
        page: "/book/guides/effects-and-capabilities/cache-at-the-edge",
        anchor: "purge-after-a-change",
    },
    Explain {
        code: "bynk.cloudflare.edge_cache_in_agent",
        blurb: "A purge reaches the edge cache only when it runs in the Worker's own `fetch`. An agent runs in a Durable Object, so an agent handler that reaches `EdgeCache`, with `given Edge` or through a provider that does, would not reach this Worker's cache. Purge from the service handler that calls the agent. A queue consumer and an `on schedule` handler have the same rule: `bynk.cloudflare.edge_cache_in_queue`, `bynk.cloudflare.edge_cache_in_schedule`.",
        example: "capability Edge = EdgeCache\n\nagent Index {\n    key name: String\n\n    store hits: Cell[Int]\n\n    on call withdraw(path: String) -> Effect[Bool] given Edge {   // ✗ an agent's purge misses the Worker's cache\n        Edge.purge([path])\n    }\n}\n\nservice operator from http {\n    on POST(\"/purge/:hex\") (hex: String) -> Effect[HttpResult[Bool]] by v: Visitor given Edge {   // ✓ purge from a service handler\n        let purged <- Edge.purge([\"/dl/\\(hex)\"])\n        Ok(purged)\n    }\n}",
        page: "/book/guides/effects-and-capabilities/cache-at-the-edge",
        anchor: "purge-after-a-change",
    },
    Explain {
        code: "bynk.cloudflare.edge_cache_in_queue",
        blurb: "A purge reaches the edge cache only from the Worker's own `fetch`, the entrypoint that serves cached responses. A queue consumer is not that entrypoint, and its purge is not known to reach the cache, so a consumer that reaches `EdgeCache`, with `given Edge` or through a provider that does, is refused. Purge from a service handler instead. An agent and an `on schedule` handler have the same rule: `bynk.cloudflare.edge_cache_in_agent`, `bynk.cloudflare.edge_cache_in_schedule`.",
        example: "capability Edge = EdgeCache\n\nservice purges from queue(\"purges\") {\n    on message(path: String) -> Effect[QueueResult] given Edge {   // ✗ not the entrypoint the cache serves\n        let purged <- Edge.purge([path])\n        if purged { Ack } else { Retry(\"refused\") }\n    }\n}\n\nservice operator from http {\n    on POST(\"/purge/:hex\") (hex: String) -> Effect[HttpResult[Bool]] by v: Visitor given Edge {   // ✓ purge from a service handler\n        let purged <- Edge.purge([\"/dl/\\(hex)\"])\n        Ok(purged)\n    }\n}",
        page: "/book/guides/effects-and-capabilities/cache-at-the-edge",
        anchor: "purge-after-a-change",
    },
    Explain {
        code: "bynk.cloudflare.edge_cache_in_schedule",
        blurb: "A purge reaches the edge cache only from the Worker's own `fetch`. Cloudflare accepts a purge from a cron run, but it clears nothing from the cache that `fetch` serves, so the stale responses would stay. An `on schedule` handler that reaches `EdgeCache`, with `given Edge` or through a provider that does, is refused. Purge from a service handler instead. An agent and a queue consumer have the same rule: `bynk.cloudflare.edge_cache_in_agent`, `bynk.cloudflare.edge_cache_in_queue`.",
        example: "capability Edge = EdgeCache\n\nservice sweep from cron {\n    on schedule(\"0 * * * *\") () -> Effect[Result[(), String]] given Edge {   // ✗ a cron purge clears nothing\n        let purged <- Edge.purge([\"/dl/\"])\n        if purged { Ok(()) } else { Err(\"refused\") }\n    }\n}\n\nservice operator from http {\n    on POST(\"/purge/:hex\") (hex: String) -> Effect[HttpResult[Bool]] by v: Visitor given Edge {   // ✓ purge from a service handler\n        let purged <- Edge.purge([\"/dl/\\(hex)\"])\n        Ok(purged)\n    }\n}",
        page: "/book/guides/effects-and-capabilities/cache-at-the-edge",
        anchor: "purge-after-a-change",
    },
    Explain {
        code: "bynk.cloudflare.edge_cache_public_authenticated",
        blurb: "In a context with `EdgeCache`, the edge keys a response by its URL alone, and `scope: public` lets it share one response with every later caller. On a route whose actor authenticates, such as a `Bearer` one, that response belongs to one caller, so it would be served to the rest. Use `scope: private`, which is never shared at the edge, or serve the route `by Visitor`. See also `bynk.http.cache_public_reads_cookies`.",
        example: "type MemberId = opaque String\n\nactor Member { auth = Bearer(secret = \"AUTH_SECRET\"), identity = MemberId }\n\n@cache(maxAge: 5.minutes, scope: public)\non GET(\"/me\") () -> Effect[HttpResult[String]] by m: Member { … }   // ✗ one member's response, shared with all\n\n@cache(maxAge: 5.minutes, scope: private)   // ✓\non GET(\"/me\") () -> Effect[HttpResult[String]] by m: Member { … }",
        page: "/book/guides/effects-and-capabilities/cache-at-the-edge",
        anchor: "mark-what-may-be-cached",
    },
    Explain {
        code: "bynk.consumes.alias_conflict",
        blurb: "`consumes payment as Pay` gives a context one name to call a unit by, and a name for one unit. Two clauses with the same alias, a unit consumed under two aliases, or an alias that is also a type, function, capability, service or agent of this context, would leave a call such as `Pay.charge(…)` with no single meaning. Give each clause its own alias, or drop it. The same clause in another file of the context is fine.",
        example: "// shop.bynk\ncontext shop\n\nconsumes payment as Pay\nconsumes shipping as Pay     // ✗ `Pay` already names `payment`\n\nconsumes payment as Pay\nconsumes shipping as Ship    // ✓ one name each",
        page: "/book/guides/program-structure/consume-services",
        anchor: "use-an-alias",
    },
    Explain {
        code: "bynk.consumes.capability_name_clash",
        blurb: "`consumes payment { Gateway }` brings `Gateway` in by its bare name, so that `given Gateway` reads it. If the context already declares a capability of that name, or a second consumed unit supplies one, the bare name would mean two things. Reach it qualified instead, `given payment.Gateway`, or rename the local one.",
        example: "// shop.bynk\ncontext shop\n\nconsumes payment { Gateway }    // ✗ this context declares a `Gateway` too\n\ncapability Gateway {\n    fn notify() -> Effect[()]\n}\n\nconsumes payment                // ✓ reach it as `payment.Gateway`",
        page: "/book/guides/effects-and-capabilities/share-across-contexts",
        anchor: "consume-it-with-a-qualified-given",
    },
    Explain {
        code: "bynk.consumes.name_conflict",
        blurb: "`consumes payment` brings every type that `payment` exports into scope under its \
                own name, because the services' signatures use them. If this context already has \
                something of that name, a type declared here or reached through `uses`, or a \
                function, asset, service, agent or provider, a mention would mean two things, so \
                the compiler refuses the clause. Rename the imported type with `as`: it is then \
                in scope only under the alias, and is still the same type. \
                `bynk.consumes.type_alias_conflict` is the same clash with an alias.",
        example: "// payment.bynk\n\
                  context payment\n\
                  \n\
                  exports transparent { Amount }\n\
                  \n\
                  // money.bynk\n\
                  commons money\n\
                  \n\
                  type Amount = Int where NonNegative\n\
                  \n\
                  // orders.bynk\n\
                  context orders\n\
                  \n\
                  uses money\n\
                  consumes payment                           // ✗ `Amount` is also in scope from `money`\n\
                  consumes payment { Amount as PayAmount }   // ✓ `PayAmount` is `payment`'s",
        page: "/book/guides/program-structure/consume-services",
        anchor: "rename-an-exported-type-that-clashes-with-one-of-yours",
    },
    Explain {
        code: "bynk.consumes.self_reference",
        blurb: "`consumes` declares that a context depends on a different one. A context's own declarations are already in scope, so consuming itself adds nothing, and a context that depends on itself is a cycle of length one. Delete the clause, or, if you meant another context, name it. A cycle through several contexts is `bynk.context.consumes_cycle`.",
        example: "// pay.bynk\ncontext pay\n\n// shop.bynk\ncontext shop\n\nconsumes shop                    // ✗ a context cannot consume itself\nconsumes pay                     // ✓ a different context",
        page: "/book/guides/program-structure/consume-services",
        anchor: "declare-the-dependency-and-call-the-service",
    },
    Explain {
        code: "bynk.consumes.service_arity",
        blurb: "A cross-context call has the shape of the `on call` handler it reaches: one argument for each parameter. The consumed context owns that signature, so the caller adapts to it. Pass the arguments the handler declares (the note says where it is declared), or change the handler's parameters if the service needs different input.",
        example: "// pay.bynk\ncontext pay\n\nservice charge {\n    on call(amount: Int) -> Effect[Int] {\n        Effect.pure(amount)\n    }\n}\n\n// shop.bynk\ncontext shop\n\nconsumes pay\n\nservice checkout {\n    on call() -> Effect[Int] {\n        pay.charge(1, 2)         // ✗ `charge` takes one argument\n    }\n}\n\nservice checkout {\n    on call() -> Effect[Int] {\n        pay.charge(1)            // ✓ one argument for `amount`\n    }\n}",
        page: "/book/guides/program-structure/consume-services",
        anchor: "declare-the-dependency-and-call-the-service",
    },
    Explain {
        code: "bynk.consumes.signature_type_unconsumed",
        blurb: "A service you call may take or return a type declared in a third context. The value crosses the call as that context's type, so your context has to state the dependency itself: it does not inherit it from the service's context. Add `consumes` for the context the error names.",
        example: "// shop/ledger.bynk\nconsumes shop.order\n\nservice domestic {\n    on call(id: String) -> Effect[Order] { … }\n}\n\n// shop/front.bynk\nconsumes shop.ledger\n\n    let o <- shop.ledger.domestic(id)    // ✗ returns `Order`, from `shop.order`\n\nconsumes shop.ledger\nconsumes shop.order                      // ✓ the dependency is stated",
        page: "/book/guides/program-structure/consume-services",
        anchor: "",
    },
    Explain {
        code: "bynk.consumes.target_is_commons",
        blurb: "`consumes` is for depending on a context, or on an adapter: it is how you call a service and use the capabilities the other unit exports. A commons has neither. It is shared types and functions, brought in with `uses`. Replace `consumes` with `uses`.",
        example: "// money.bynk\ncommons money\n\ntype Amount = Int where NonNegative\n\n// shop.bynk\ncontext shop\n\nconsumes money                   // ✗ `money` is a commons\nuses money                       // ✓ shared declarations come in with `uses`",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "the-two-kinds-of-module",
    },
    Explain {
        code: "bynk.consumes.type_alias_capability",
        blurb: "`as` renames a type that the consumed context exports, for use in this one. A capability keeps its own name wherever it is used, in `given` and in calls. Select it bare, `consumes payment { Gateway }`, or consume the context without braces and write `given payment.Gateway`.",
        example: "// payment.bynk\ncontext payment\n\nexports capability { Gateway }\n\n// shop.bynk\ncontext shop\n\nconsumes payment { Gateway as Pay }   // ✗ a capability cannot be aliased\n\nconsumes payment { Gateway }          // ✓ select it bare",
        page: "/book/guides/program-structure/consume-services",
        anchor: "rename-an-exported-type-that-clashes-with-one-of-yours",
    },
    Explain {
        code: "bynk.consumes.type_alias_conflict",
        blurb: "An alias is a name in this context, so it cannot be one that is already taken: by another alias, by a type or function the context declares or brings in with `uses`, or by a capability. Give each aliased type a name of its own. A consumed type that is not aliased, and clashes with one of these, is `bynk.consumes.name_conflict`.",
        example: "// shop.bynk\ncontext shop\n\nconsumes payment { AuthId as Id, Receipt as Id }            // ✗ `Id` already names `AuthId`\n\nconsumes payment { AuthId as Id, Receipt as ReceiptId }     // ✓ distinct aliases",
        page: "/book/guides/program-structure/consume-services",
        anchor: "rename-an-exported-type-that-clashes-with-one-of-yours",
    },
    Explain {
        code: "bynk.consumes.type_alias_repeated",
        blurb: "A type is in scope under one name, so it is selected once in a file: the alias is that name. Selecting it twice, or both bare and aliased, would leave two names for it. Keep one selection. The bare selection of a type is not needed, since `consumes payment` already brings every exported type into scope.",
        example: "// shop.bynk\ncontext shop\n\nconsumes payment { AuthId as A, AuthId as B }   // ✗ `AuthId` aliased twice\n\nconsumes payment { AuthId as Auth }             // ✓ one alias",
        page: "/book/guides/program-structure/consume-services",
        anchor: "rename-an-exported-type-that-clashes-with-one-of-yours",
    },
    Explain {
        code: "bynk.consumes.type_alias_unknown_export",
        blurb: "`consumes payment { AuthId as Auth }` brings a type that `payment` exports in under another name, so the type has to be one it exports. A misspelt name, or a type that `payment` keeps to itself, is the usual cause. Correct the name, or ask `payment` to export the type.",
        example: "// shop.bynk\ncontext shop\n\nconsumes payment { AuthIdd as Auth }   // ✗ `payment` exports no `AuthIdd`\n\nconsumes payment { AuthId as Auth }    // ✓ a type it exports",
        page: "/book/guides/program-structure/consume-services",
        anchor: "rename-an-exported-type-that-clashes-with-one-of-yours",
    },
    Explain {
        code: "bynk.consumes.unknown_context",
        blurb: "A `consumes` clause is how one context states that it depends on another, so it has to name a context in the project, and no unit of that name exists. Check the spelling against the `context` line of the unit you mean.",
        example: "// pay.bynk\ncontext pay\n\n// shop.bynk\ncontext shop\n\nconsumes paymnet                 // ✗ no context `paymnet`\nconsumes pay                     // ✓ the context's declared name",
        page: "/book/guides/program-structure/consume-services",
        anchor: "declare-the-dependency-and-call-the-service",
    },
    Explain {
        code: "bynk.consumes.unknown_service",
        blurb: "A call such as `pay.charge(…)` reaches a service of the consumed context, and `pay` has none by that name. Only a service's `on call` handler can be called across contexts, so check the spelling against the consumed context and that the service has an `on call` handler. A call with the wrong number of arguments is `bynk.consumes.service_arity`.",
        example: "// pay.bynk\ncontext pay\n\nservice charge {\n    on call(amount: Int) -> Effect[Int] {\n        Effect.pure(amount)\n    }\n}\n\n// shop.bynk\ncontext shop\n\nconsumes pay\n\nservice checkout {\n    on call() -> Effect[Int] {\n        pay.charg(1)             // ✗ `pay` has no service `charg`\n    }\n}\n\nservice checkout {\n    on call() -> Effect[Int] {\n        pay.charge(1)            // ✓ the declared name\n    }\n}",
        page: "/book/guides/program-structure/consume-services",
        anchor: "declare-the-dependency-and-call-the-service",
    },
    Explain {
        code: "bynk.context.consumes_cycle",
        blurb: "Contexts may not form a circle of `consumes` clauses, whether directly or \
                through others. `bynk deploy` takes the order in which to upload the Workers \
                from the `consumes` clauses, because Cloudflare resolves a Service Binding when \
                a Worker is uploaded and so the callee must already exist; a cycle has no such \
                order. The error names the cycle and sits on one of its clauses. Remove one \
                clause: usually the callee takes what it needs as an argument instead of calling \
                back. Logic both contexts need can move into a `commons` that each `uses`.",
        example: "// a.bynk\n\
                  context a\n\
                  \n\
                  consumes b                       // ✗ `a` and `b` consume each other (reported on one clause)\n\
                  \n\
                  // b.bynk\n\
                  context b\n\
                  \n\
                  consumes a                       // ✗\n\
                  \n\
                  service quote {                  // ✓ no `consumes a`: `quote` takes `rate` as an argument\n    on call(rate: Int) -> Effect[Int] {\n        Effect.pure(rate * 2)\n    }\n\
                  }",
        page: "/book/guides/projects-build-and-deployment/deploy-to-cloudflare",
        anchor: "",
    },
    Explain {
        code: "bynk.context.external_construction",
        blurb: "A context that `consumes` another may hold and pass on the other's types, but \
                may not build one. A refined or opaque type can promise an invariant, a \
                `Matches` pattern or a smart constructor, and that holds only if every value is \
                made by the owner; `.of`, `.unsafe` and a variant constructor are refused from \
                outside, as is the record syntax of an opaque type. Ask the owner for the value \
                instead: a service that returns it, or takes the plain input and does the check. \
                `bynk.context.opaque_inspection` is the other side, looking inside such a value.",
        example: "// payment.bynk\n\
                  context payment\n\
                  \n\
                  exports transparent { AuthId }\n\
                  \n\
                  type AuthId = String where Matches(\"AUTH-[0-9]+\")\n\
                  \n\
                  service issue {\n    on call() -> Effect[AuthId] { … }\n\
                  }\n\
                  \n\
                  // orders.bynk\n\
                  context orders\n\
                  \n\
                  consumes payment\n\
                  \n\
                  AuthId.of(s)             // ✗ only `payment` makes an `AuthId`\n\
                  \n\
                  payment.issue()          // ✓ ask `payment` for one",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.context.external_decode",
        blurb: "Decoding creates a value, and a context may not create one of a type another \
                context owns and exports opaque. A decoder would check no more than the base \
                type, because the owner's predicate is the owner's secret. So a handler that \
                takes such a type as its input (an HTTP, queue, event or WebSocket handler \
                decodes it from bytes), and a `Json.decode`, `Json.decodeSql` or `Codable` \
                witness of it, are refused. Take the plain form of the input and let the owner's \
                service make the value. A reply from the owner's own service is fine: its decode \
                is generated.",
        example: "// pay.bynk\n\
                  context pay\n\
                  \n\
                  exports opaque { Token }\n\
                  \n\
                  type Token = opaque String where Matches(\"T-[0-9]+\")\n\
                  \n\
                  // shop.bynk\n\
                  context shop\n\
                  \n\
                  consumes pay\n\
                  \n\
                  service api from http {\n    on POST(\"/t\") (body: Token) -> Effect[HttpResult[String]] by v: Visitor {     // ✗ `Token` cannot be decoded here\n    on POST(\"/t\") (body: String) -> Effect[HttpResult[String]] by v: Visitor {    // ✓ take the plain input",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.context.external_provider",
        blurb: "A provider with no body (`provides Clock = SystemClock`) says its implementation comes from elsewhere, and only an `adapter` has a binding to supply it. In a context every provider is written in Bynk, so it needs a body implementing the capability's operations. Add one, or move the capability into an adapter if the code is TypeScript. The reverse mistake is `bynk.adapter.provider_has_body`.",
        example: "provides Clock = SystemClock             // ✗ no body, and a context has no binding\n\nprovides Clock = SystemClock {           // ✓ implemented in Bynk\n    fn now() -> Effect[Int] {\n        Effect.pure(0)\n    }\n}",
        page: "/book/reference/capabilities",
        anchor: "providing-a-capability",
    },
    Explain {
        code: "bynk.context.opaque_inspection",
        blurb: "A context that writes `exports opaque { T }` lets other contexts hold a `T`, pass it on and \
                call its methods, but not look inside it: they cannot `match` on it, read a field, \
                match a record pattern against it, or name one of its variants. Only the owning \
                context sees the shape, so it can change the representation, or keep an invariant \
                the type's constructors enforce, without breaking a consumer. Ask the owner for \
                what you need instead: a service that returns the answer as a plain value, such as \
                a `Bool`. If the shape is meant to be public, the owner exports it with \
                `exports transparent { T }`.",
        example: "// pay.bynk\n\
                  context pay\n\n\
                  exports opaque { Status }        // the owner hides the shape\n\n\
                  type Status = | Paid | Declined\n\n\
                  // shop.bynk\n\
                  context shop\n\n\
                  consumes pay\n\n\
                  service checkout {\n    on call() -> Effect[Bool] {\n        let s <- pay.charge(1)\n        match s {                      // ✗ `Status` is opaque here, so its variants are hidden\n            Paid => Effect.pure(true)\n            Declined => Effect.pure(false)\n        }\n    }\n}\n\n\
                  // pay.bynk\n\
                  exports transparent { Status }   // ✓ the owner shares the shape",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.context.rebrand_construction",
        blurb: "A context that `uses` a commons gets its sum types by name, but not their variant constructors, so `Circle(1.0)` is refused there: the context re-exports the type under its own name, without the constructors. Build the value in the commons, in a function that returns it, and call that function from the context.",
        example: "// geo/shapes.bynk\ncommons geo.shapes\n\ntype Shape = | Circle(radius: Float) | Square(side: Float)\n\n// app/main.bynk\ncontext app.main\n\nuses geo.shapes\n\nfn unit() -> Shape {\n    Circle(1.0)                          // ✗ constructors are not in scope here\n}\n\n// geo/shapes.bynk\nfn unitCircle() -> Shape { Circle(1.0) } // ✓ built in the commons\n\n// app/main.bynk\nfn unit() -> Shape {\n    unitCircle()                         // ✓ called from the context\n}",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.contract.duplicate_name",
        blurb: "Every `requires` and `ensures` clause has a name, and a \
                failure report points at a clause by that name, so each name \
                must be unique within one function, across both kinds. Rename \
                the repeat; the error is on its name.",
        example: "fn inc(x: Int) -> Int\n\
                  \x20\x20requires bigger: x > 0\n\
                  \x20\x20ensures bigger: result > x      // ✗ `bigger` is already taken\n\n\
                  fn inc(x: Int) -> Int\n\
                  \x20\x20requires positive: x > 0\n\
                  \x20\x20ensures bigger: result > x      // ✓",
        page: "/book/reference/testing",
        anchor: "contracts",
    },
    Explain {
        code: "bynk.contract.impure_predicate",
        blurb: "A contract clause is checked on every call in the dev and test \
                build, and by the runner on generated inputs, so its predicate \
                must be a pure `Bool` over the parameters and `result`. \
                Effects (`Effect.pure`, `?`) and test-only constructs \
                (`expect`, `val`, observations, `faults`, `trace`) cannot \
                appear in it. State the claim as a plain comparison, and keep \
                assertions and effects in a `case`.",
        example: "fn inc(x: Int) -> Int\n\
                  \x20\x20ensures bigger: expect result > x   // ✗ `expect` is a test statement\n\n\
                  fn inc(x: Int) -> Int\n\
                  \x20\x20ensures bigger: result > x          // ✓ a pure `Bool`",
        page: "/book/reference/testing",
        anchor: "contracts",
    },
    Explain {
        code: "bynk.contract.not_bool",
        blurb: "A `requires` or `ensures` clause is a claim the function can \
                meet or break, so its predicate must be a `Bool`. A value of \
                another type, such as an `Int`, claims nothing that could \
                fail. Write a comparison or another `Bool` expression over the \
                parameters (and `result`, in an `ensures`).",
        example: "fn inc(x: Int) -> Int\n\
                  \x20\x20ensures bigger: result + 1      // ✗ an `Int`, not a `Bool`\n\n\
                  fn inc(x: Int) -> Int\n\
                  \x20\x20ensures bigger: result > x      // ✓",
        page: "/book/reference/testing",
        anchor: "contracts",
    },
    Explain {
        code: "bynk.contract.restated_by_test",
        blurb: "A function's `ensures` is checked at every call, in production and in every test that reaches it. A test that calls the function and `expect`s the same predicate adds nothing and has to be kept in step with the contract. The compiler flags the repeat. Delete the test, or keep a case only for a specific value a reader should see, such as `expect discount(100, 10) == 90`.",
        example: "// demo/c.bynk\nfn discount(p: Int, pct: Int) -> Int\n    ensures never_above: result <= p\n    { … }\n\n// tests/c.test.bynk\nlet r = discount(100, 10)\nexpect r <= 100    // ✗ repeats `never_above`\nexpect r == 90     // ✓ a specific witnessed value",
        page: "/book/reference/testing",
        anchor: "contracts",
    },
    Explain {
        code: "bynk.contract.result_in_requires",
        blurb: "A `requires` clause is checked on entry, before the function \
                has returned anything, so `result` is not bound there. Only an \
                `ensures` can read it. Either state the condition over the \
                parameters, or, if it is a claim about the return value, make \
                it an `ensures`. A parameter that is itself named `result` is \
                read as that parameter.",
        example: "fn inc(x: Int) -> Int\n\
                  \x20\x20requires small: result < 100    // ✗ nothing has been returned yet\n\n\
                  fn inc(x: Int) -> Int\n\
                  \x20\x20ensures small: result < 100     // ✓ checked on exit",
        page: "/book/reference/testing",
        anchor: "contracts",
    },
    Explain {
        code: "bynk.cron.bad_params",
        blurb: "A scheduled trigger carries one thing, the time it fired, so an `on schedule` handler takes at most one parameter, and it must be an `Int`: that time in milliseconds since the epoch. Drop the parameter, or type it `Int`. To act on a richer value, build it from the time inside the body.",
        example: "on schedule(\"*/5 * * * *\") (label: String) -> Effect[Result[(), String]] { … }   // ✗ the parameter is the time\n\non schedule(\"*/5 * * * *\") (at: Int) -> Effect[Result[(), String]] { … }          // ✓",
        page: "/book/reference/cron",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.cron.duplicate_schedule",
        blurb: "The context's `scheduled` entry point picks the handler by the schedule that fired, so each schedule may be declared once across all its `from cron` services. Two handlers with the same one would leave it with two to run. Merge them into one handler, or give one a different schedule.",
        example: "on schedule(\"*/5 * * * *\") () -> Effect[Result[(), String]] { … }\non schedule(\"*/5 * * * *\") () -> Effect[Result[(), String]] { … }   // ✗ already scheduled\n\non schedule(\"0 * * * *\") () -> Effect[Result[(), String]] { … }     // ✓ a different schedule",
        page: "/book/reference/cron",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.cron.invalid_schedule",
        blurb: "A cron schedule is a standard five-field expression: minute, hour, day of month, month, day of week, separated by whitespace. Bynk checks the field count at compile time so a schedule the platform's cron trigger would reject is caught before deploy, not when it silently never runs. Only the count is checked at compile time. A bad parameter or return type on the handler is `bynk.cron.bad_params` or `bynk.cron.return_not_effect_result`.",
        example: "on schedule(\"every day\") () -> Effect[Result[(), String]] { … }   // ✗ words, not five fields\n\non schedule(\"0 3 * * *\") () -> Effect[Result[(), String]] { … }   // ✓ 03:00 every day",
        page: "/book/reference/cron",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.cron.return_not_effect_result",
        blurb: "A cron run has no caller to hand a value to and no retry channel: `Ok(())` completes the run silently and `Err(e)` logs `e` and completes. So the handler must return `Effect[Result[(), E]]`, with `()` on the success side, for some error type `E`. Return `Ok(())` when the work is done and map a failure to `Err`. The queue counterpart is `bynk.queue.return_not_queue_result`.",
        example: "on schedule(\"*/5 * * * *\") () -> Effect[Result[Int, String]] { … }   // ✗ a cron run has nowhere to put an `Int`\n\non schedule(\"*/5 * * * *\") () -> Effect[Result[(), String]] { … }   // ✓ unit on success",
        page: "/book/reference/cron",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.d1.select_field_missing",
        blurb: "`select[T]` and `firstAs[T]` read each row into the record `T`, matching columns to \
                fields by name, so every field needs a column of that name. `Json.decodeSql` reads \
                a literal `all` or `first` the same way, for each required field. Rename the \
                field, or alias the column in the SQL (`… AS price`) so the names agree.",
        example: "type Order = { id: String, price: Int }\nlet _ <- Orders.select[Order](\"FROM orders\", [])    // ✗ `orders` has no column `price`\n\ntype Order = { id: String, total: Int }\nlet _ <- Orders.select[Order](\"FROM orders\", [])    // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "sql-checked-against-the-migrations",
    },
    Explain {
        code: "bynk.d1.sql_invalid",
        blurb: "Bynk prepares each literal SQL statement of a `D1` capability against the schema \
                your migrations build, applied to an empty in-memory SQLite, so a syntax error or an \
                unknown table or column is a compile error, not a failure on the first request. \
                Nothing runs. Correct the statement, or add the migration that creates what it \
                names. A project whose schema is made elsewhere can turn the check off with \
                `[check] d1_sql = false`.",
        example: "let _ <- Orders.all(\"SELECT id, totl FROM orders\", [])     // ✗ no such column: totl\n\nlet _ <- Orders.all(\"SELECT id, total FROM orders\", [])    // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "sql-checked-against-the-migrations",
    },
    Explain {
        code: "bynk.d1.sql_param_count",
        blurb: "D1 refuses a statement bound to a different number of values than it has \
                parameters, so Bynk counts them at compile time by SQLite's rule: the highest `?N`, \
                with a bare `?` taking the next number. It checks a list written out in place, or a \
                name `let`-bound to one. Pass one `SqlValue` for each parameter, or change the SQL.",
        example: "let _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1 AND total > ?2\", [Text(id)])                 // ✗ two parameters, one value\n\nlet _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1 AND total > ?2\", [Text(id), Integer(100)])    // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "sql-checked-against-the-migrations",
    },
    Explain {
        code: "bynk.dependency.path_outside_root",
        blurb: "Your own project's path dependencies can point anywhere, but a dependency's own path dependencies are honoured only when they stay inside the workspace: a path out of it names a directory that need not exist where the package is used. A fetched package's path dependencies are never honoured. Move the target into the workspace.",
        example: "# lib/bynk.toml, a dependency of the project\n[dependencies]\ncore = { package = \"acme/core\", path = \"../../outside\" }   // ✗ outside the workspace\n\ncore = { package = \"acme/core\", path = \"../core\" }         // ✓ a workspace member",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "what-the-checks-catch",
    },
    Explain {
        code: "bynk.detach.in_agent_handler",
        blurb: "`detach` stops waiting for an effect and hands it to the execution context, so something must keep the effect alive after the handler returns. A service handler has an execution context that does. An agent handler runs inside the agent's own object, with no such context, and its effects are awaited so that they finish with the handler. So `detach` is refused there, and in a closure there. Await the effect instead, with `do` or `let _ <-`, or move the `detach` to the service handler that calls the agent. A `detach` of a store operation is `bynk.detach.store_op`.",
        example: "on call bump() -> Effect[()] given Logger {\n    do Logger.info(\"bump\").detach()   // ✗ an agent handler cannot detach\n    do count.update((n) => n + 1)\n}\n\n\
                  on call bump() -> Effect[()] given Logger {\n    do Logger.info(\"bump\")             // ✓ awaited, so it finishes with the handler\n    do count.update((n) => n + 1)\n}",
        page: "/book/reference/capabilities",
        anchor: "detach",
    },
    Explain {
        code: "bynk.detach.in_pure_context",
        blurb: "`detach` hands a running effect to the execution context, and only an effectful body has effects to run: a pure function never runs one. Change the enclosing function's return type to `Effect[…]`. The same rule holds for `bynk.within.in_pure_context`, `bynk.race.in_pure_context` and `bynk.send.in_pure_context`.",
        example: "fn twice(ping: Effect[()], n: Int) -> Int {\n    let _ = ping.detach()   // ✗ the body is pure\n    n * 2\n}\n\n\
                  fn twice(ping: Effect[()], n: Int) -> Effect[Int] {   // ✓ an effectful body\n    do ping.detach()\n    Effect.pure(n * 2)\n}",
        page: "/book/reference/capabilities",
        anchor: "detach",
    },
    Explain {
        code: "bynk.detach.store_op",
        blurb: "In an agent handler, a store write is not work to detach: it is staged and committed atomically with the rest of the handler, so there is nothing to leave running. `detach` is refused there for any effect, and a store operation gets its own code to point at the fix. Drop the `.detach()` and write `do field.op(…)`; the write is committed with the handler. For a `detach` that is not of a store operation see `bynk.detach.in_agent_handler`.",
        example: "on call bump() -> Effect[()] {\n    do count.update((n) => n + 1).detach()   // ✗ a store write is not detached\n}\n\n\
                  on call bump() -> Effect[()] {\n    do count.update((n) => n + 1)            // ✓ staged, committed with the handler\n}",
        page: "/book/reference/capabilities",
        anchor: "detach",
    },
    Explain {
        code: "bynk.duration.literal_overflow",
        blurb: "`5.minutes` is a `Duration`, which the compiler stores as a whole number of milliseconds, so it multiplies the count by the unit when it reads the literal. A count whose total does not fit in a 64-bit integer is refused rather than wrapped into a wrong or negative span. No real timeout or interval is that large: write a smaller count, or a smaller unit.",
        example: "fn forever() -> Duration { 9007199254740991.days }   // ✗ overflows the millisecond range\n\nfn forever() -> Duration { 365.days }                // ✓",
        page: "/book/reference/types",
        anchor: "duration",
    },
    Explain {
        code: "bynk.effect.bind_in_pure_context",
        blurb: "`<-` waits for an effect, so it is only allowed in a body that is itself effectful, one whose return type is `Effect[T]`. A function that returns a plain `Int` promises that it waits for nothing and reaches nothing outside, and a `<-` inside it would break that promise for every caller. A `par` block awaits too, and is refused in the same place. Change the return type to `Effect[T]` and return `Effect.pure(…)`, or compute the value without an effect. Calling a function value that returns an `Effect` is `bynk.effect.fn_value_in_pure_context`.",
        example: "fn next(n: Int) -> Int {\n    let x <- Effect.pure(n)      // ✗ `<-` in a function that returns a plain `Int`\n    x\n}\n\nfn next(n: Int) -> Effect[Int] {   // ✓ effectful, so it may bind with `<-`\n    let x <- Effect.pure(n)\n    Effect.pure(x)\n}",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "",
    },
    Explain {
        code: "bynk.effect.bind_on_non_effect",
        blurb: "`let x <- e` waits for `e` and binds its result, so `e` must be an `Effect[T]`. A plain value has nothing to wait for, and `<-` on one usually means the right-hand side is not the call you meant. Write `let x = …` for a pure binding. Where an effect is required and you hold a plain value, wrap it with `Effect.pure(…)`. A `<-` in a body that is not itself effectful is `bynk.effect.bind_in_pure_context`.",
        example: "fn run() -> Effect[Int] {\n    let x <- 42                  // ✗ `42` is an `Int`, not an `Effect[Int]`\n    Effect.pure(x)\n}\n\nfn run() -> Effect[Int] {\n    let x = 42                   // ✓ a pure binding\n    Effect.pure(x)\n}",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "why-effects-are-gated",
    },
    Explain {
        code: "bynk.effect.capability_in_pure_context",
        blurb: "Calling a capability operation is an effect, and effects are only allowed in a body that is itself effectful, one that returns `Effect[T]`, so that a type tells you whether a body reaches outside. A lambda checked against a plain function type such as `(Int) -> Int` is a pure body, so a capability call inside it is refused even when the handler around it is effectful. Make the call in the handler, bind it with `<-`, and pass the result in. A call the `given` clause does not cover is `bynk.given.undeclared_capability`.",
        example: "let add: (Int) -> Int = (n) => {\n    let t = Clock.now()          // ✗ a capability call in a pure lambda\n    n\n}\n\nlet t <- Clock.now()             // ✓ the handler is effectful, so it makes the call\nlet add: (Int) -> Int = (n) => n + t",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "why-effects-are-gated",
    },
    Explain {
        code: "bynk.effect.cross_context_in_pure_context",
        blurb: "A call to a service in a consumed context is an effect, as a capability call is, so it is only allowed in a body that is itself effectful. A handler is effectful, but a lambda inside it is a body of its own, and a service call alone does not make it effectful. Make the call in the handler body, bind it with `<-`, and use the result in the lambda.",
        example: "on call() -> Effect[Int] {\n    let f = (n: Int) => pay.charge(n)   // ✗ a service call in a pure lambda\n    Effect.pure(1)\n}\n\non call() -> Effect[Int] {\n    let paid <- pay.charge(5)           // ✓ the handler makes the call\n    let f = (n: Int) => n + paid\n    Effect.pure(f(1))\n}",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "why-effects-are-gated",
    },
    Explain {
        code: "bynk.effect.do_discards_result",
        blurb: "`do e` awaits an effect and discards what it returns, which is fine for a unit or an ordinary value but not for a `Result`: an `Err` dropped this way is lost without a trace. Bind it and handle the error with `let r <- e` and a `match` (or `?`). If dropping the outcome is really what you want, write `let _ <- e`, which is legal and keeps the discard visible in the source. `do` on a plain value is `bynk.effect.do_on_non_effect`.",
        example: "fn send() -> Effect[Result[Int, String]] {\n    Effect.pure(Ok(1))\n}\n\nfn run() -> Effect[()] {\n    do send()                    // ✗ would silently drop an `Err`\n}\n\nfn run() -> Effect[()] {\n    let _ <- send()              // ✓ the discard is written out\n    Effect.pure(())\n}",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "why-effects-are-gated",
    },
    Explain {
        code: "bynk.effect.do_in_pure_context",
        blurb: "`do e` runs an effect as a statement, so like `<-` it belongs in a body that is itself effectful, one whose return type is `Effect[T]`. A function that returns a plain `Int` promises to wait for nothing and reach nothing outside, and a `do` inside it would break that promise for every caller. Change the return type to `Effect[T]` and end with `Effect.pure(…)`, or drop the `do`. The same mistake with `<-` is `bynk.effect.bind_in_pure_context`.",
        example: "fn ping() -> Effect[()] {\n    Effect.pure(())\n}\n\nfn run() -> Int {\n    do ping()                    // ✗ `do` in a function that returns a plain `Int`\n    5\n}\n\nfn run() -> Effect[Int] {        // ✓ effectful, so `do` is allowed\n    do ping()\n    Effect.pure(5)\n}",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "why-effects-are-gated",
    },
    Explain {
        code: "bynk.effect.do_on_non_effect",
        blurb: "`do e` performs an effect, so its operand must be an `Effect[T]`: a call to something effectful. A plain value has nothing to run, which usually means the operand is not the call you meant. Call the effectful function you intended. If you wanted the value, bind it with `let`. An effect whose result is a `Result` is `bynk.effect.do_discards_result`, and `<-` on a plain value is `bynk.effect.bind_on_non_effect`.",
        example: "fn ping() -> Effect[()] {\n    Effect.pure(())\n}\n\nfn clear() -> Effect[()] {\n    do 5                         // ✗ `5` is an `Int`, not an `Effect[T]`\n}\n\nfn clear() -> Effect[()] {\n    do ping()                    // ✓ an effect\n}",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "why-effects-are-gated",
    },
    Explain {
        code: "bynk.effect.fn_value_in_pure_context",
        blurb: "Calling a function value whose type returns `Effect[T]` is an effect, just as calling a \
                capability is, and effects are only allowed in a body that is itself effectful, one \
                whose return type is an `Effect`. Bynk makes effects explicit so that a function's \
                signature tells you whether it can touch the outside world: a function that returns a \
                plain `Int` is pure and can be reasoned about as such. A call through a parameter \
                would hide an effect behind that signature, so it is refused. Either make the function \
                return `Effect[T]` and bind the call with `<-`, or take a pure function instead. A \
                capability call from a handler that has not declared it is a different mistake, \
                `bynk.given.undeclared_capability`.",
        example: "fn sneak(f: Int -> Effect[Int], x: Int) -> Int {\n    \
                    let y = f(x)                          // ✗ an effect inside a pure function\n    \
                    x\n}\n\n\
                  fn sneak(f: Int -> Effect[Int], x: Int) -> Effect[Int] {   // ✓ the function is effectful\n    \
                    let y <- f(x)\n    \
                    Effect.pure(y)\n}",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "",
    },
    Explain {
        code: "bynk.effect.unbound_effect",
        blurb: "Bynk starts an effectful call as soon as it is built, so an `Effect[T]` that is built and then \
                dropped still runs, unawaited, racing every statement after it, and nothing in the \
                program shows it. Effects are meant to happen in the order you wrote them, so in an \
                effectful body an `Effect` value must be bound with `let x <- …`, run with `do …` (for \
                an `Effect[()]`), or returned as the body's value. The error fires where an `Effect` is \
                built in a value position that cannot await it: the right-hand side of a plain `let` \
                (including `let _ = …`), a list element, and the payload of `Some`, `Ok`, `Err`, a \
                variant or a record field. An argument to another call is left alone, because the \
                parameter decides what happens to it. If you want the result, use `<-`; if only the \
                effect, use `do`.",
        example: "on call() -> Effect[()] {\n    \
                    let e = Counter(\"k\").bump()       // ✗ started, never awaited\n    \
                    Effect.pure(())\n}\n\n\
                  on call() -> Effect[()] {\n    \
                    do Counter(\"k\").bump()           // ✓ run in order\n    \
                    Effect.pure(())\n}",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "",
    },
    Explain {
        code: "bynk.email.bad_params",
        blurb: "An email `on message` handler takes the message as one `InboundEmail` parameter: inbound email delivers one message per call, so the handler is `on message(mail: InboundEmail) -> Effect[EmailVerdict]`. A parameter of another type, or none, or more than one, is refused. Import `InboundEmail` and `EmailVerdict` from `bynk.cloudflare`.",
        example: "service support from email(\"support@shop.example\") {\n    on message(mail: String) -> Effect[EmailVerdict] {   // ✗ not an `InboundEmail`\n        Accept\n    }\n}\n\nservice support from email(\"support@shop.example\") {\n    on message(mail: InboundEmail) -> Effect[EmailVerdict] {   // ✓\n        Accept\n    }\n}",
        page: "/book/reference/inbound-email",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.email.duplicate_address",
        blurb: "Inbound mail is dispatched to a service by its envelope recipient, so each address belongs to one service. Addresses compare without regard to case. A literal address and the catch-all `*@domain` for the same domain may both be claimed: the literal address wins, and the catch-all takes everything else. Merge the two handlers, or claim different addresses.",
        example: "service support from email(\"support@shop.example\") { … }\nservice helpdesk from email(\"Support@shop.example\") { … }   // ✗ the same address\n\nservice helpdesk from email(\"help@shop.example\") { … }   // ✓ its own address",
        page: "/book/reference/inbound-email",
        anchor: "addresses",
    },
    Explain {
        code: "bynk.email.duplicate_handler",
        blurb: "A `from email` service claims one address and decides each message it is given, so it has exactly one `on message` handler. To treat two addresses differently, declare two services, each with its own address.",
        example: "service support from email(\"support@shop.example\") {\n    on message(mail: InboundEmail) -> Effect[EmailVerdict] { Accept }\n    on message(mail: InboundEmail) -> Effect[EmailVerdict] { Accept }   // ✗ a second handler\n}\n\nservice support from email(\"support@shop.example\") {\n    on message(mail: InboundEmail) -> Effect[EmailVerdict] { Accept }   // ✓ one handler\n}",
        page: "/book/reference/inbound-email",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.email.invalid_address",
        blurb: "A `from email` service claims a bare address, `local@domain`, or a whole domain with `*@domain`. The domain is dotted labels of letters, digits and `-`. A display name (`Support <a@b.example>`), a list, a wildcard inside the local part, and an address with spaces or quotes are refused: Email Routing matches the bare address.",
        example: "service support from email(\"Support <support@shop.example>\") { … }   // ✗ a display name\n\nservice support from email(\"support@shop.example\") { … }            // ✓",
        page: "/book/reference/inbound-email",
        anchor: "addresses",
    },
    Explain {
        code: "bynk.email.return_not_email_verdict",
        blurb: "An email handler decides what happens to the message, and the decision is named: `Accept`, `Reject(reason)`, `Forward(to)` or `Reply(message)`, the `EmailVerdict` sum from `bynk.cloudflare`. It is not a `Result` or a `String`, because the decision is independent of success or failure. It is the agency rule that gives a queue handler `QueueResult`.",
        example: "on message(mail: InboundEmail) -> Effect[String] {         // ✗ not a verdict\n    \"done\"\n}\n\non message(mail: InboundEmail) -> Effect[EmailVerdict] {   // ✓\n    Accept\n}",
        page: "/book/reference/inbound-email",
        anchor: "the-verdict",
    },
    Explain {
        code: "bynk.embed.binary_content",
        blurb: "An embed becomes a `String` in the compiled code, so its file must be text. A file \
                holding a NUL byte is binary, and a `String` made from it would be meaningless. \
                Binary data belongs in `assets/`, where the platform serves it as a file. Text \
                that is not valid UTF-8 is `bynk.embed.invalid_encoding`.",
        example: "embed logo = \"embeds/logo.png\"    // ✗ the file has a NUL byte\n\nasset logo = \"assets/logo.png\"    // ✓ a binary file is served, not read",
        page: "/book/reference/embeds",
        anchor: "what-the-text-is",
    },
    Explain {
        code: "bynk.embed.duplicate",
        blurb: "An embed is a `String` constant used by its name, so a unit declares each name once: \
                a second `embed shell` would leave `shell` meaning two files. Give each embed its \
                own name. An embed whose name is an asset's, a function's or a type's is \
                `bynk.resolve.name_conflict` instead.",
        example: "embed shell = \"embeds/page.html\"\nembed shell = \"embeds/other.html\"     // ✗ `shell` is already declared\n\nembed shell = \"embeds/page.html\"\nembed footer = \"embeds/other.html\"    // ✓ one name each",
        page: "/book/reference/embeds",
        anchor: "declaring-an-embed",
    },
    Explain {
        code: "bynk.embed.invalid_encoding",
        blurb: "An `embed` becomes a `String`, which is Unicode text, so the file must be valid \
                UTF-8. A file saved in another encoding, such as Latin-1, holds bytes that are not \
                UTF-8 and cannot be read as text. Re-save it as UTF-8. A file that holds a NUL byte \
                is binary and is `bynk.embed.binary_content`; a binary file belongs in `assets/`, as \
                an `asset`.",
        example: "embed shell = \"embeds/page.html\"        // ✗ the file is Latin-1, not UTF-8\n\
                embed shell = \"embeds/page.html\"        // ✓ once the file is saved as UTF-8",
        page: "/book/reference/embeds",
        anchor: "what-the-text-is",
    },
    Explain {
        code: "bynk.embed.missing_file",
        blurb: "An `embed` compiles a file's text into the program as a `String`, so the file must \
                exist when the project is built, or there is nothing to compile in. The path is \
                relative to the project root and must name a regular file under `embeds/`; a \
                symbolic link is not followed. Create the file, or correct the path. A file that \
                exists but cannot be read is `bynk.embed.unreadable_file`.",
        example: "embed shell = \"embeds/page.html\"        // ✗ no such file in the project\n\
                embed shell = \"embeds/page.html\"        // ✓ once embeds/page.html exists",
        page: "/book/reference/embeds",
        anchor: "declaring-an-embed",
    },
    Explain {
        code: "bynk.embed.outside_context_or_commons",
        blurb: "An `embed` is compiled into the Worker of each context that declares it, or that `uses` a commons that does. An adapter is the host boundary, not a Worker, so it has nowhere to carry the text, and an `embed` there is refused. Declare it in a context or a commons, and have the context use that. For a file the platform serves rather than compiles in, see `bynk.asset.outside_context_or_commons`.",
        example: "// app/host.bynk\nadapter app.host\n\nbinding \"./host.binding.ts\"\n\nembed shell = \"embeds/page.html\"        // ✗ an adapter cannot embed\n\n// app/page.bynk\ncommons app.page\n\nembed shell = \"embeds/page.html\"        // ✓ in a commons",
        page: "/book/reference/embeds",
        anchor: "declaring-an-embed",
    },
    Explain {
        code: "bynk.embed.outside_embeds_dir",
        blurb: "An embed is compiled into the deployed code, so it may come only from the project's \
                `embeds/` folder. If any path were allowed, a declaration could pull `.env` or a \
                source file into the bundle just by naming it. Move the file into `embeds/` and \
                name it there. A path that is empty, absolute or contains `..` is \
                `bynk.embed.unsafe_path` instead.",
        example: "embed shell = \"templates/page.html\"    // ✗ not in the embeds/ folder\n\nembed shell = \"embeds/page.html\"       // ✓",
        page: "/book/reference/embeds",
        anchor: "declaring-an-embed",
    },
    Explain {
        code: "bynk.embed.unsafe_path",
        blurb: "An embed's path is joined onto the project root to find the file, so it must stay \
                inside the project: it may not be empty or absolute, and it may not contain a `..` \
                segment, which could climb out of it. Write the path relative to the project root, \
                inside `embeds/`. A relative path that is only outside `embeds/` is \
                `bynk.embed.outside_embeds_dir`.",
        example: "embed shell = \"embeds/../page.html\"    // ✗ a `..` segment\n\nembed shell = \"embeds/page.html\"       // ✓",
        page: "/book/reference/embeds",
        anchor: "declaring-an-embed",
    },
    Explain {
        code: "bynk.event.bad_params",
        blurb: "An `on event` handler takes the event payload and, optionally, a second parameter of type `EventEnvelope`, which carries the emission's metadata: its event id, publisher, time and schema version. Any other count, or any other type in second place, has nothing to receive it. Remove the extra parameter or type it `EventEnvelope`. A wrong first parameter is `bynk.event.handler_param_type_mismatch`.",
        example: "consumes bynk\n\nevent Paid = { id: String }\n\nservice onPaid from Events(Paid) {\n  on event(e: Paid, env: String) -> Effect[()] {   // ✗ the second parameter must be `EventEnvelope`\n  on event(e: Paid, env: EventEnvelope) -> Effect[()] {   // ✓",
        page: "/book/guides/events/understand-events",
        anchor: "the-envelope-and-idempotent-handling",
    },
    Explain {
        code: "bynk.event.bad_schema_dispatch",
        blurb: "`via schema(N)` filters a subscription by the envelope's `schemaVersion`. Versions count up from 1, so every bound must be a positive `Int` literal: `schema(0)` or `schema(0..3)` names a version no event has. Write a version, a range such as `schema(2..4)`, or `_` for every version. A range whose lower bound is above its upper one is `bynk.types.empty_range_pattern`.",
        example: "service OnPaid from Events(Paid) via schema(0) {    // ✗ versions start at 1\nservice OnPaid from Events(Paid) via schema(1) {    // ✓\n    on event(e: Paid) -> Effect[()] { … }\n}",
        page: "/book/guides/events/understand-events",
        anchor: "dispatching-by-version-with-via-scheman",
    },
    Explain {
        code: "bynk.event.bad_schema_version",
        blurb: "`@schema(N)` states the version an event is at, so that the number can be reviewed in the source and is checked against the version the registry computes. `N` must be one positive `Int` literal, given by position, and `@schema` may appear once on an event: `@schema(0)`, `@schema(\"2\")`, `@schema(v: 2)` and a second `@schema` are all refused. Write `@schema(2)`, or drop the annotation, since the registry tracks the version anyway.",
        example: "event Paid @schema(0) = {    // ✗ versions start at 1\nevent Paid @schema(1) = {    // ✓\n    id: String,\n}\n\nconst Version: Int = 1\nevent Refunded @schema(Version) = {    // ✓ an `Int` constant bound to a literal\n    id: String,\n}",
        page: "/book/guides/events/understand-events",
        anchor: "asserting-a-schema-version-with-scheman",
    },
    Explain {
        code: "bynk.event.duplicate_handler",
        blurb: "A `from Events(E)` service has exactly one `on event` handler. To react to an event in two ways, declare two services, each subscribing to it: every subscriber is delivered to independently. So the second handler is refused here, and the fix is to move it into a service of its own.",
        example: "service onPaid from Events(Paid) {\n    on event(e: Paid) -> Effect[()] { … }\n    on event(e: Paid) -> Effect[()] { … }   // ✗ a service has one `on event` handler\n}\n\nservice onPaidAudit from Events(Paid) {     // ✓ a second subscriber, delivered to separately\n    on event(e: Paid) -> Effect[()] { … }\n}",
        page: "/book/guides/events/understand-events",
        anchor: "the-three-pieces",
    },
    Explain {
        code: "bynk.event.emit_not_an_event",
        blurb: "`Events.emit[E]` takes an `event` type. A plain `type` declared in the same context is refused: nothing could subscribe to it, since a `from Events(…)` header names events, so the emission would silently go nowhere. Declare the type with `event` instead, or name the event you meant. A name that is not declared in this context at all is `bynk.event.emit_outside_owner`.",
        example: "type Receipt = { id: String }\nevent Paid = { id: String }\n\nEvents.emit[Receipt](Receipt { id: id })   // ✗ `Receipt` is a `type`, not an `event`\nEvents.emit[Paid](Paid { id: id })         // ✓ an `event` of this context",
        page: "/book/guides/events/understand-events",
        anchor: "only-the-declaring-context-may-emit",
    },
    Explain {
        code: "bynk.event.emit_outside_owner",
        blurb: "Only the context that declares an event may emit it. A consumer can see the event through `consumes`, to subscribe to it with `from Events(…)`, but being able to name a type does not make it yours to forge: other contexts then could not trust that an event came from its owner. Emit an event of your own, or call a service of the owning context and let it emit.",
        example: "// orders.bynk\ncontext orders\n\nexports transparent { Paid }\n\nevent Paid = { id: String }\n\n// mailer.bynk\ncontext mailer\n\nconsumes orders\nconsumes bynk { Events }\n\nevent Sent = { id: String }\n\nEvents.emit[Paid](Paid { id: \"x\" })   // ✗ `Paid` belongs to `orders`\nEvents.emit[Sent](Sent { id: \"x\" })   // ✓ `Sent` is declared here",
        page: "/book/guides/events/understand-events",
        anchor: "only-the-declaring-context-may-emit",
    },
    Explain {
        code: "bynk.event.handler_param_type_mismatch",
        blurb: "An `on event` handler's first parameter is the event itself, so its type must be the one the `from Events(E)` header names. A subscription pattern is checked against that event, and the body reads `e` at that type. Make the parameter `E`, or change the header to the event you mean to handle. The optional second parameter is `EventEnvelope`; other mistakes there are `bynk.event.bad_params`.",
        example: "event Paid = { id: String }\nevent Refunded = { id: String }\n\nservice onPaid from Events(Paid) {\n    on event(e: Refunded) -> Effect[()] { … }   // ✗ the header says `Paid`\n    on event(e: Paid) -> Effect[()] { … }       // ✓\n}",
        page: "/book/guides/events/understand-events",
        anchor: "the-three-pieces",
    },
    Explain {
        code: "bynk.event.outside_context",
        blurb: "An `event` is declared in a context, and only that context may emit it, so an event belongs to its owner. A `commons` is shared code that other units `uses`, with no owner to emit from, so one may not declare an event. Declare the event in the context that raises it. Other contexts subscribe by consuming that context.",
        example: "commons shop\n\nevent Paid = { id: String }   // ✗ a commons has no owner to emit it\n\ncontext shop\n\nevent Paid = { id: String }   // ✓ declared where it is emitted",
        page: "/book/guides/events/understand-events",
        anchor: "the-three-pieces",
    },
    Explain {
        code: "bynk.event.pattern_duplicate_field",
        blurb: "A subscription pattern narrows delivery by field: `from Events(Paid { region: Domestic, .. })` runs the handler only when `region` is `Domestic`. The listed fields combine with AND, so naming one twice would ask for two values at once, or say the same thing twice, and is refused rather than guessed at. List each field once. To accept several values, leave the field out of the pattern and `match` on it in the handler.",
        example: "service OnPaid from Events(Paid { region: Domestic, region: International, .. }) {    // ✗ `region` twice\nservice OnPaid from Events(Paid { region: Domestic, .. }) {                            // ✓\n    on event(e: Paid) -> Effect[()] { … }\n}",
        page: "/book/guides/events/understand-events",
        anchor: "filtering-delivery-with-a-pattern",
    },
    Explain {
        code: "bynk.event.pattern_type_mismatch",
        blurb: "A subscription pattern compares a field with a literal or a variant, so the value must be of the field's type: an `Int` literal for an `Int` field, a `String` for a `String`, a variant of the field's own sum type. A value of any other type could never match an emission. Write one of the field's type.",
        example: "event Paid = { id: String, amount: Int }\n\nservice onPaid from Events(Paid { amount: \"10\", .. }) {   // ✗ `amount` is an `Int`\n\nservice onPaid from Events(Paid { amount: 10, .. }) {   // ✓",
        page: "/book/guides/events/understand-events",
        anchor: "filtering-delivery-with-a-pattern",
    },
    Explain {
        code: "bynk.event.pattern_unknown_field",
        blurb: "A subscription pattern filters on the event's own fields, so each name in it must be one the event declares. A name that matches none could never filter anything, and is usually a misspelling; the diagnostic lists the declared fields. Use one of them.",
        example: "event Paid = { id: String, amount: Int }\n\nservice onPaid from Events(Paid { total: 10, .. }) {   // ✗ `Paid` has no field `total`\n\nservice onPaid from Events(Paid { amount: 10, .. }) {   // ✓",
        page: "/book/guides/events/understand-events",
        anchor: "filtering-delivery-with-a-pattern",
    },
    Explain {
        code: "bynk.event.pattern_unknown_variant",
        blurb: "A pattern that tests a sum-typed field names a variant of that type, and an emission can only carry the variants the type declares. A name it lacks, often a misspelling, could never match. Use a variant the type declares. A variant that carries a payload is refused too, as `bynk.event.pattern_variant_payload`.",
        example: "type Region =\n    | Domestic\n    | International\n    | Refunded(reason: String)\nevent Paid = { id: String, region: Region }\n\nservice onPaid from Events(Paid { region: Region.Nowhere, .. }) {   // ✗ `Region` has no `Nowhere`\n\nservice onPaid from Events(Paid { region: Region.Domestic, .. }) {   // ✓",
        page: "/book/guides/events/understand-events",
        anchor: "filtering-delivery-with-a-pattern",
    },
    Explain {
        code: "bynk.event.pattern_variant_payload",
        blurb: "A pattern can test only which variant a field holds, not what it carries. For a variant with a payload that would admit emissions the pattern looks as if it excludes, so only payload-free variants are allowed. Subscribe to a payload-free variant, or drop the pattern and read the field in the handler's body.",
        example: "type Region =\n    | Domestic\n    | International\n    | Refunded(reason: String)\nevent Paid = { id: String, region: Region }\n\nservice onPaid from Events(Paid { region: Region.Refunded, .. }) {   // ✗ `Refunded` carries a `reason`\n\nservice onPaid from Events(Paid { region: Region.Domestic, .. }) {   // ✓ no payload",
        page: "/book/guides/events/understand-events",
        anchor: "filtering-delivery-with-a-pattern",
    },
    Explain {
        code: "bynk.event.return_not_effect_unit",
        blurb: "Emission is fire-and-forget: nothing receives a subscriber's result, so an `on event` handler returns `Effect[()]` and anything else is refused. Return `Effect.pure(())` once its work is done, and move a value it computes into an effect the handler performs, such as a write. A handler that does not return an `Effect` at all is `bynk.service.return_not_effect`.",
        example: "service onPaid from Events(Paid) {\n    on event(e: Paid) -> Effect[Int] { … }   // ✗ the result has nowhere to go\n    on event(e: Paid) -> Effect[()] { … }    // ✓\n}",
        page: "/book/guides/events/understand-events",
        anchor: "emission-is-fire-and-forget-and-release-at-commit",
    },
    Explain {
        code: "bynk.event.unknown_annotation",
        blurb: "The only annotation an event accepts is `@schema(N)`, which asserts its schema version. Names like `@indexed` belong to agent state and mean nothing on an event, so a stray or misspelt one would be silently ignored if it were not refused. Remove the annotation, or use `@schema` with a positive integer.",
        example: "event Paid @indexed = { id: String }   // ✗ not an event annotation\n\nevent Paid @schema(1) = { id: String }   // ✓",
        page: "/book/guides/events/understand-events",
        anchor: "asserting-a-schema-version-with-scheman",
    },
    Explain {
        code: "bynk.event.unknown_subscription",
        blurb: "A `from Events(E)` header must name an event declared in this context or in a context it `consumes`. A name that matches neither would be a subscription that never receives anything, and nothing would say so; it is almost always a misspelling, or a `consumes` that is missing. Fix the name, or add `consumes` for the context whose `event` it is.",
        example: "context mailer\n\nconsumes orders\n\nservice onPaid from Events(Payd) {   // ✗ no event `Payd`\n\nservice onPaid from Events(Paid) {   // ✓ the event `orders` declares",
        page: "/book/guides/events/understand-events",
        anchor: "the-three-pieces",
    },
    Explain {
        code: "bynk.exec.unavailable",
        blurb: "A function that uses `~>`, `detach`, `within` or `race` hands work to the execution context, which a service handler has. An agent handler, a WebSocket handler, a provider operation, a function that is not effectful and a `const`, `store` or predicate initialiser do not. Calling such a function from one of those, directly or through other functions, or naming it as a value, is refused. The note gives the chain of calls down to the construct. Make the call in a service handler, and pass what it returns on.",
        example: "fn bounded(e: Effect[Int]) -> Effect[Bool] { … }\n\nagent Counter {\n    key id: String\n    store n: Cell[Int] = 0\n\n    on call check() -> Effect[Bool] {\n        bounded(Effect.pure(1))    // ✗ an agent handler has no execution context\n    }\n}\n\nservice api {\n    on call() -> Effect[Bool] {\n        bounded(Effect.pure(1))    // ✓ a service handler has\n    }\n}",
        page: "/book/reference/capabilities",
        anchor: "within",
    },
    Explain {
        code: "bynk.expect.faults_not_effect",
        blurb: "`expect <call> faults` claims that an effectful call throws, for \
                instance because a `stub … fails`. Only a call can fault, so the \
                subject must be the call itself, with type `Effect[_]`. A value \
                already bound, or a pure method on one, has nothing left to throw. \
                Write the call inside the claim, not a name bound from it. A fault \
                that is not claimed fails the case.",
        example: "// tests/vault.test.bynk\n\
                  let label = \"sealed\"\n\
                  expect label.length() faults   // ✗ a pure method, not an effectful call\n\
                  expect box.call() faults       // ✓ the call that faults",
        page: "/book/reference/testing",
        anchor: "faults",
    },
    Explain {
        code: "bynk.expect.not_bool",
        blurb: "`expect` states a claim that is checked when the case runs, so its \
                predicate must be a `Bool`: true, or the case fails. A count or a \
                record is not a claim. Compare the value with what you expect \
                (`==`, `<`, `is`), or use a method that returns `Bool`. It is the \
                same predicate surface that `invariant` and `ensures` use.",
        example: "// demo/count.bynk\n\
                  fn bump(n: Int) -> Int { n + 1 }\n\
                  \n\
                  // tests/count.test.bynk\n\
                  expect bump(1)         // ✗ an Int is not a claim\n\
                  expect bump(1) == 2    // ✓ compare it",
        page: "/book/reference/testing",
        anchor: "expect",
    },
    Explain {
        code: "bynk.expect.outside_case",
        blurb: "`expect` is a test claim, checked when a `case` runs. Production \
                code has no test runner to report a failure to, so an `expect` in \
                a function, a handler or anywhere else outside a `case` body is \
                refused. To check a condition in running code, return a `Bool` or \
                a `Result`; to check it in a test, move the `expect` into a case. \
                A `requires` or `ensures` contract is the always-on form.",
        example: "// demo/rules.bynk\n\
                  fn positive(n: Int) -> Bool {\n\
                  \x20   expect n > 0           // ✗ not in a case\n\
                  \x20   n > 0\n\
                  }\n\
                  \n\
                  // tests/rules.test.bynk\n\
                  expect positive(1)       // ✓ in a case",
        page: "/book/reference/testing",
        anchor: "expect",
    },
    Explain {
        code: "bynk.exports.capability_not_provided",
        blurb: "`exports capability { Clock }` lets other contexts depend on `Clock` and have it \
                built for them, so the exporting context must also say how: a `provides Clock = \
                … { … }` declaration. Without one, a consumer would hold a capability that \
                nothing in the project can instantiate. Add the provider. (A derived capability, \
                `capability Name = Kind`, cannot be exported at all; that is \
                `bynk.exports.derived_capability`.)",
        example: "exports capability { Clock }       // ✗ nothing provides `Clock`\n\
                  \n\
                  capability Clock {\n    fn now() -> Effect[Int]\n\
                  }\n\
                  \n\
                  provides Clock = Fixed {           // ✓\n    fn now() -> Effect[Int] { … }\n\
                  }",
        page: "/book/guides/effects-and-capabilities/share-across-contexts",
        anchor: "export-the-capability-from-the-providing-context",
    },
    Explain {
        code: "bynk.exports.conflicting_visibility",
        blurb: "A type is exported `opaque`, so consuming contexts can pass it around but not look inside it, or `transparent`, so they can. It cannot be both, and the compiler will not pick for you. Keep the clause you mean and delete the other. The same visibility twice is `bynk.exports.duplicate_export`.",
        example: "exports opaque { OrderId }\nexports transparent { OrderId }    // ✗ already exported `opaque`\n\ntype OrderId = String where Matches(\"ORD-[0-9]+\")\n\nexports opaque { OrderId }         // ✓ one visibility",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.exports.derived_capability",
        blurb: "A derived capability is an instance of a platform resource, such as a KV namespace, and its data belongs to the context that declares it. Exporting it would let another context read and write that data directly. Keep it out of `exports capability`; another context reaches the data through this context's services.",
        example: "exports capability { Sessions }\n\ncapability Sessions = Kv   // ✗ an instance cannot be exported\n\ncapability Sessions = Kv   // ✓ private to this context",
        page: "/book/reference/capabilities",
        anchor: "derived-capabilities",
    },
    Explain {
        code: "bynk.exports.duplicate_export",
        blurb: "A type or capability is exported once. Naming it again in a second `exports` clause with the same visibility adds nothing, and usually means a clause was pasted twice, or a name was meant for another one. Delete the repeat. Within a single clause the repeat is `bynk.exports.duplicate_in_clause`, and the same type under both `opaque` and `transparent` is `bynk.exports.conflicting_visibility`.",
        example: "exports transparent { Order }\nexports transparent { Order }    // ✗ already exported above\n\ntype Order = { total: Int }\n\nexports transparent { Order }    // ✓ exported once",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.exports.duplicate_in_clause",
        blurb: "An `exports` clause lists each type once. Naming one twice is a typo or a pasted name, and the second may have been meant for another type. Delete the repeat, or correct the name. The same type in two separate clauses is `bynk.exports.duplicate_export`.",
        example: "exports transparent { Order, Order }    // ✗ `Order` listed twice\n\ntype Order = { total: Int }\n\nexports transparent { Order }           // ✓ listed once",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.exports.undeclared_capability",
        blurb: "`exports capability { Clock }` offers a capability of this context to the contexts that consume it, so the capability has to be declared in this context, and provided there too (`bynk.exports.capability_not_provided`). A name that is not declared is usually a misspelling. Correct it, or declare the capability here. A name in a type `exports` clause is `bynk.exports.undeclared_type`.",
        example: "exports capability { Cloc }      // ✗ no capability `Cloc` in this context\n\ncapability Clock {\n    fn now() -> Effect[Int]\n}\n\nexports capability { Clock }     // ✓ the declared name\n\nprovides Clock = Fixed {\n    fn now() -> Effect[Int] { Effect.pure(0) }\n}",
        page: "/book/guides/effects-and-capabilities/share-across-contexts",
        anchor: "export-the-capability-from-the-providing-context",
    },
    Explain {
        code: "bynk.exports.undeclared_type",
        blurb: "An `exports` clause lists types this context declares, to make them visible to the contexts that consume it. A name the context does not declare has nothing to export: it is usually a misspelling, or a type that is declared elsewhere. Correct the name, or declare the type in this context. A name in `exports capability` that is not declared is `bynk.exports.undeclared_capability`.",
        example: "exports opaque { Ordr }          // ✗ no type `Ordr` in this context\n\ntype Order = { total: Int }\n\nexports opaque { Order }         // ✓ the declared name",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.fixture.binary_content",
        blurb: "A `fixture` becomes a `String` in the test code, so its file must be text. A file holding a NUL byte is binary, and a `String` made from it would be meaningless. Keep a text form of the data in the fixture, such as base64, and decode it in the case with `Bytes.fromBase64`. Text that is not valid UTF-8 is `bynk.fixture.invalid_encoding`.",
        example: "fixture blob = \"fixtures/blob.bin\"    // ✗ the file has a NUL byte\n\nfixture blob = \"fixtures/blob.b64\"    // ✓ its base64 text",
        page: "/book/reference/fixtures",
        anchor: "what-the-text-is",
    },
    Explain {
        code: "bynk.fixture.duplicate",
        blurb: "A fixture is a `String` constant used by its name in the suite, so a suite declares each name once: a second `fixture note` would leave `note` meaning two files. Give each fixture its own name. A fixture whose name is a declaration already in scope is `bynk.resolve.name_conflict` instead.",
        example: "fixture note = \"fixtures/a.txt\"\nfixture note = \"fixtures/b.txt\"      // ✗ `note` is already declared\n\nfixture note = \"fixtures/a.txt\"\nfixture other = \"fixtures/b.txt\"     // ✓ one name each",
        page: "/book/reference/fixtures",
        anchor: "declaring-a-fixture",
    },
    Explain {
        code: "bynk.fixture.invalid_encoding",
        blurb: "A `fixture` becomes a `String`, which is Unicode text, so its file must be valid UTF-8. A file saved in another encoding, such as Latin-1, holds bytes that cannot be read as text. Re-save it as UTF-8. A file that holds a NUL byte is binary and is `bynk.fixture.binary_content`.",
        example: "fixture note = \"fixtures/note.txt\"    // ✗ the file is Latin-1, not UTF-8\nfixture note = \"fixtures/note.txt\"    // ✓ once the file is saved as UTF-8",
        page: "/book/reference/fixtures",
        anchor: "what-the-text-is",
    },
    Explain {
        code: "bynk.fixture.missing_file",
        blurb: "A `fixture` reads a file into a `String` for the suite's \
                cases, so the file must exist as a regular file in the \
                project's `fixtures/` folder. A misspelt name, a file not yet \
                added, or a symbolic link (which is not followed) leaves the \
                suite without its input. Add the file or correct the path. A \
                path outside that folder is \
                `bynk.fixture.outside_fixtures_dir`; an empty, absolute or \
                `..` path is `bynk.fixture.unsafe_path`.",
        example: "// tests/desk.test.bynk\n\
                  fixture note = \"fixtures/absent.txt\"    // ✗ no such file in `fixtures/`\n\
                  fixture note = \"fixtures/note.txt\"      // ✓",
        page: "/book/reference/fixtures",
        anchor: "declaring-a-fixture",
    },
    Explain {
        code: "bynk.fixture.outside_fixtures_dir",
        blurb: "A fixture is test input, and it may come only from the project's `fixtures/` folder. If any path were allowed, a declaration could read `.env` or a source file just by naming it. Move the file into `fixtures/` and name it there. A path that is empty, absolute or contains `..` is `bynk.fixture.unsafe_path` instead.",
        example: "fixture note = \"data/note.txt\"        // ✗ not in the fixtures/ folder\n\nfixture note = \"fixtures/note.txt\"    // ✓",
        page: "/book/reference/fixtures",
        anchor: "declaring-a-fixture",
    },
    Explain {
        code: "bynk.fixture.unsafe_path",
        blurb: "A fixture's path is joined onto the project root to find the file, so it must stay inside the project: it may not be empty or absolute, and it may not contain a `..` segment, which could climb out of it. Write the path relative to the project root, inside `fixtures/`. A relative path that is only outside `fixtures/` is `bynk.fixture.outside_fixtures_dir`.",
        example: "fixture note = \"fixtures/../note.txt\"    // ✗ a `..` segment\n\nfixture note = \"fixtures/note.txt\"       // ✓",
        page: "/book/reference/fixtures",
        anchor: "declaring-a-fixture",
    },
    Explain {
        code: "bynk.generics.ambiguous_instantiation",
        blurb: "A generic function with an `Ord` bound, named as a value, is instantiated where it stands, from the function type expected there. When that type is still open, because the receiving parameter's own type is not fixed yet, there is no `T` to choose, and a guess could order by the wrong type. Wrap the function in a lambda, which is checked after the other arguments, or annotate the expected function type.",
        example: "fn same[T: Ord](x: T) -> T { x }\n\nfn wrap[A](g: (A) -> A, x: A) -> A { g(x) }\n\nfn pick() -> Int { wrap(same, 1) }              // ✗ `A` is known only from the later argument\n\nfn pick() -> Int { wrap((n) => same(n), 1) }    // ✓ a lambda is checked after it",
        page: "/book/reference/generics",
        anchor: "ord",
    },
    Explain {
        code: "bynk.generics.bound_on_capability_op",
        blurb: "A capability operation is carried out by its provider, written in TypeScript, so it has no Bynk body for a bound to serve; an `Ord` bound would also add a hidden comparator argument. Only `Key` and `Codable` are accepted, and both are handled at the call site. Remove the bound, and compare in the code that calls the operation.",
        example: "capability Registry {\n    fn lookup[K: Key + Ord](key: K) -> Effect[Option[String]]   // ✗ only `Key` and `Codable`\n}\n\ncapability Registry {\n    fn lookup[K: Key](key: K) -> Effect[Option[String]]         // ✓\n}",
        page: "/book/reference/generics",
        anchor: "capability-operations-take-key-and-codable",
    },
    Explain {
        code: "bynk.generics.codable_op_return",
        blurb: "A `Codable` parameter of a capability operation is encoded at the call site, so the provider receives plain JSON. Decoding the provider's reply back into `T` would have to say what a failed decode of stored data means, and that is not settled, so `T` may not appear in the return type. Return plain data, such as a `String`, and decode it where the call is made.",
        example: "capability Store {\n    fn load[T: Codable](id: String) -> Effect[Option[T]]   // ✗ `T` in the return type\n}\n\ncapability Store {\n    fn load(id: String) -> Effect[Option[String]]          // ✓ plain data back\n}",
        page: "/book/reference/generics",
        anchor: "capability-operations-take-key-and-codable",
    },
    Explain {
        code: "bynk.generics.duplicate_type_param",
        blurb: "The type parameters of one declaration are told apart by name, and a repeat would collapse into a single parameter, so `Pair[T, T]` would force both fields to the same type with no sign of it. Give each parameter its own name. A method's own type parameter may not reuse a name from its receiver type either, as that would make the two ambiguous.",
        example: "type Pair[T, T] = { a: T, b: T }    // ✗ `T` declared twice\n\ntype Pair[A, B] = { a: A, b: B }    // ✓ one name for each",
        page: "/book/reference/generics",
        anchor: "type-parameters",
    },
    Explain {
        code: "bynk.generics.generic_non_record",
        blurb: "Type parameters are for a type whose shape can mention them: a record, `{ value: T }`, or a sum whose variants carry payloads. A refined, opaque or alias type has a fixed base, so a parameter would have nowhere to go. Give the type a record or sum body that uses its parameters, or drop the parameters.",
        example: "type Wrapped[T] = Int            // ✗ `T` has nowhere to be used\n\ntype Wrapped[T] = { value: T }   // ✓ a record may be generic",
        page: "/book/reference/types",
        anchor: "generic-record-types",
    },
    Explain {
        code: "bynk.generics.generic_sum_embeds",
        blurb: "`embeds E as V` lets `?` wrap an `E` error in the variant `V`, picking the variant by the error's type. In a generic sum that conversion would have to hold for every instantiation, which is not supported. Drop the `embeds` clause and wrap the error by hand where it is needed, with `.mapErr(Payment)`, or make the sum non-generic.",
        example: "type PayError = enum { Declined }\n\ntype Failure[T] =\n    | Payment(reason: PayError)\n    | Other(value: T)\n    embeds PayError as Payment    // ✗ a generic sum cannot embed\n\ntype Failure[T] =\n    | Payment(reason: PayError)\n    | Other(value: T)             // ✓ no `embeds`; wrap with `.mapErr(Payment)`",
        page: "/book/reference/types",
        anchor: "generic-sums",
    },
    Explain {
        code: "bynk.generics.method_on_generic_type",
        blurb: "A static method on a generic type (one with no `self`) is generic over the type's parameters, and \
                a call has to be able to fix each of them: from its arguments, from the type the call is \
                expected to have, or from explicit type arguments. If the method's parameter and return \
                types never mention one of the type's parameters, nothing in a call can determine it, so \
                the declaration is refused once, here, rather than every call being ambiguous. Mention \
                the parameter in the signature, or make the method a free function. An instance method \
                (with `self`) is not affected, since the receiver carries the type. A call that leaves a \
                parameter open anyway is `bynk.generics.uninferable_type_arg`.",
        example: "type Box[T] = { value: T }\n\n\
                  fn Box.empty() -> Box[Int] {              // ✗ never mentions `T`\n    \
                    Box { value: 0 }\n}\n\n\
                  fn Box.empty() -> Box[Option[T]] {        // ✓ `T` appears in the return type\n    \
                    Box { value: None }\n}",
        page: "/book/reference/generics",
        anchor: "static-methods-on-generic-types",
    },
    Explain {
        code: "bynk.generics.missing_codable_bound",
        blurb: "`Json.encode` and `Json.decode` work only on types that cross the JSON boundary, and a type \
                parameter might be a function, an `Effect` or another type that does not. So a type \
                parameter that is encoded or decoded, or passed to a `Codable`-bounded parameter of \
                another generic or of a capability operation, must be declared `T: Codable` (or \
                `where A: Codable` for a receiver type's parameter), and a call must instantiate it with a \
                type that is `Codable`. At run time the bound costs a hidden codec argument, so a refined \
                type is still revalidated on decode. `Codable` does not imply `Eq`, so the diagnostic's \
                fix writes `T: Eq + Codable` beside an existing `Eq`. It is the `Json` counterpart of \
                `bynk.generics.missing_eq_bound`, `bynk.generics.missing_ord_bound` and \
                `bynk.generics.missing_key_bound`.",
        example: "fn encode[T](v: T) -> String {\n    \
                    Json.encode(v)                  // ✗ `T` could be a function\n}\n\n\
                  fn encode[T: Codable](v: T) -> String {\n    \
                    Json.encode(v)                  // ✓ the bound is in the signature\n}",
        page: "/book/reference/generics",
        anchor: "codable",
    },
    Explain {
        code: "bynk.generics.missing_eq_bound",
        blurb: "`==` and `!=` work on a type parameter only if the function says so with an `Eq` bound. \
                A type parameter stands for any type, and some types cannot be compared: a function, an \
                `Effect`, a `Query`, a `Stream` or a held `Connection`. The bound puts that requirement \
                in the signature, where a caller, hover and signature help can see it, instead of leaving \
                it to fail inside the body. The same rule applies when a generic passes its own parameter \
                to an `Eq`-bounded parameter of another generic, so a bound appears at every link of a \
                chain of calls. Declare `T: Eq`; `T: Ord` and `T: Key` also allow `==`. The \
                diagnostic's fix adds the bound.",
        example: "fn same[T](a: T, b: T) -> Bool {\n    a == b                  // ✗ `T` could be a function\n}\n\n\
                  fn same[T: Eq](a: T, b: T) -> Bool {\n    a == b                  // ✓ the bound is in the signature\n}",
        page: "/book/reference/generics",
        anchor: "bounds",
    },
    Explain {
        code: "bynk.generics.missing_key_bound",
        blurb: "A `Map` key or a `Set` element must be a type that can be held under a canonical encoding, and \
                a type parameter might be a function or another type that cannot. So a type parameter \
                that reaches a `Map` key or a `Set` element, in a signature, a generic type's field, or \
                a `distinct`, `groupBy` or join key, must be declared `K: Key`, and a call must \
                instantiate it with a key type. `Key` implies `Eq`, so an existing `Eq` bound is \
                replaced rather than joined (`K: Eq + Key` is a warning). The diagnostic's fix declares \
                the bound, and `bynk check --fix` applies it across a project.",
        example: "fn single[K](k: K) -> Map[K, Int] {                 // ✗ `K` has no `Key` bound\n    Map.empty().insert(k, 1)\n}\n\n\
                  fn single[K: Key](k: K) -> Map[K, Int] {            // ✓ declared\n    Map.empty().insert(k, 1)\n}",
        page: "/book/reference/generics",
        anchor: "key",
    },
    Explain {
        code: "bynk.generics.missing_ord_bound",
        blurb: "`<`, `<=`, `>` and `>=` work on a type parameter only if the function says so with an `Ord` bound. \
                A type parameter stands for any type, and many types have no total order: a function, an \
                `Effect`, a `Bool`, a record that does not `derives Ord`. The bound puts the requirement \
                in the signature, where a caller, hover and signature help can see it, and a call must \
                instantiate it with an orderable type. The same rule covers an ordering key and a \
                parameter passed on to an `Ord`-bounded parameter of another generic, so the bound \
                appears at every link of a chain of calls. Declare `T: Ord`; it implies `Eq`, so an \
                existing `Eq` is replaced (`T: Eq + Ord` is only a warning). The diagnostic's fix adds \
                the bound.",
        example: "fn bigger[T](a: T, b: T) -> Bool {\n    \
                    a > b                            // ✗ `T` has no order\n}\n\n\
                  fn bigger[T: Ord](a: T, b: T) -> Bool {\n    \
                    a > b                            // ✓ the bound is in the signature\n}",
        page: "/book/reference/generics",
        anchor: "ord",
    },
    Explain {
        code: "bynk.generics.polymorphic_recursion",
        blurb: "A generic type may refer to itself only uniformly: the self-reference must pass \
                the declaration's own type parameters through unchanged and in order, as \
                `Weird[T]` inside `Weird[T]`. Anything else is polymorphic recursion, such as \
                `Weird[List[T]]` inside `Weird[T]`, or `Pair[B, A]` inside `Pair[A, B]`, and it \
                generates an unbounded set of instantiations, so it is refused at the \
                declaration. The same holds for mutually recursive generics: a partner may be \
                applied only to the declaration's own parameters or to types that mention none, \
                never to a type that wraps one of them.",
        example: "type Weird[T] = { value: T, next: Option[Weird[List[T]]] }   // ✗ a different instantiation\n\n\
                  type Weird[T] = { value: T, next: Option[Weird[T]] }         // ✓ parameters passed through",
        page: "/book/guides/type-system/recursive-generic-types",
        anchor: "",
    },
    Explain {
        code: "bynk.generics.recursive_generic_at_boundary",
        blurb: "A recursive generic type, a record or sum that contains itself, declares and works \
                inside a context, but it has no codec yet. So it is refused wherever a codec is \
                required: a service or agent handler signature, an agent `store` field or key, an \
                event field, an actor identity type, a `Json.encode` or `Json.decode` target, or a \
                `Codable` witness. The error fires once per use, and when the type is reached \
                through another type, its note names the field path. To cross a boundary today, \
                use a concrete recursive type, or keep the generic one inside the context.",
        example: "type Tree[A] =\n    | Leaf\n    | Node(left: Tree[A], value: A, right: Tree[A])\n\n\
                  service s {\n    on call(t: Tree[Int]) -> Effect[Int] { … }   // ✗ no codec for a recursive generic\n}\n\n\
                  type IntTree =\n    | Leaf\n    | Node(left: IntTree, value: Int, right: IntTree)\n\n\
                  service s {\n    on call(t: IntTree) -> Effect[Int] { … }     // ✓ a concrete recursive type\n}",
        page: "/book/guides/type-system/recursive-generic-types",
        anchor: "where-it-cannot-go-yet",
    },
    Explain {
        code: "bynk.generics.type_arg_count",
        blurb: "A generic type is applied to exactly as many type arguments as it declares parameters: `Box[T]` takes one. Too many, too few, arguments on a type that is not generic, and a generic type named with no arguments at all are all refused, since each argument fills one parameter. A type parameter such as `T` is not itself generic, so `T[Int]` is refused too. Give the arguments the declaration asks for.",
        example: "type Box[T] = { value: T }\n\nfn open(b: Box[Int, String]) -> Int {   // ✗ `Box` takes one type argument\n    0\n}\n\nfn open(b: Box[Int]) -> Int {           // ✓\n    0\n}",
        page: "/book/reference/generics",
        anchor: "type-parameters",
    },
    Explain {
        code: "bynk.generics.type_arg_mismatch",
        blurb: "The type arguments of a generic call do not fit. The checker infers each type parameter from \
                the arguments, and an argument that contradicts what an earlier one \
                settled is refused: `pick(1, \"one\")` cannot make `T` both `Int` and `String`. The same \
                code covers explicit type arguments of the wrong number (`pick[Int, Int](…)`), type \
                arguments given to a function that is not generic, and a type parameter named like a \
                declared type. Make the arguments agree, give the type argument explicitly where the \
                inference is not what you meant, or rename the parameter.",
        example: "fn pick[T](a: T, b: T) -> T {\n    a\n}\n\n\
                  let x = pick(1, \"one\")          // ✗ `T` is `Int` from the first argument\n\n\
                  pick(1, 2)                      // ✓ both arguments are `Int`",
        page: "/book/reference/generics",
        anchor: "type-parameters",
    },
    Explain {
        code: "bynk.generics.uninferable_type_arg",
        blurb: "A call to a generic function leaves a type parameter undetermined. Bynk fixes a type parameter \
                from the arguments and, failing that, from the type the call is expected to have; it \
                never guesses. `none()` has no argument that mentions `T`, and `let x = none()` gives it \
                no expected type, so there is nothing to choose `T` from. The error also fires when a \
                lambda's parameter types depend on a parameter that the other arguments leave open. Give \
                the type argument explicitly with `none[Int]()`, or annotate the binding so the expected \
                type supplies it: `let x: Option[Int] = none()`.",
        example: "fn none[T]() -> Option[T] {\n    None\n}\n\n\
                  let x = none()                    // ✗ nothing says what `T` is\n\n\
                  let x = none[Int]()               // ✓ given explicitly",
        page: "/book/reference/generics",
        anchor: "the-expected-type-for-every-generic-call",
    },
    Explain {
        code: "bynk.generics.unknown_constraint",
        blurb: "A bound after a type parameter, `T: C`, must name one of the four constraints Bynk has: `Eq`, \
                `Ord`, `Key` and `Codable`. There are no user-defined constraints or interfaces, so a name \
                like `Hashable` or `Comparable` is refused rather than silently ignored; ignoring it \
                would let a function compile that promised more than the compiler could check. The same \
                code is reported for a `derives` clause that names anything but `Ord`; naming `Eq`, \
                `Key` or `Codable` there is `bynk.types.derives_automatic`, because they hold \
                automatically. Pick the constraint that matches what the body does: `Eq` for `==`, `Ord` \
                for ordering, `Key` for a `Map` key or `Set` element, `Codable` for `Json`.",
        example: "fn first[A: Hashable](x: A) -> A {       // ✗ `Hashable` is not a constraint\n    \
                    x\n}\n\n\
                  fn first[A: Key](x: A) -> A {            // ✓ one of `Eq`, `Ord`, `Key`, `Codable`\n    \
                    x\n}",
        page: "/book/reference/generics",
        anchor: "bounds",
    },
    Explain {
        code: "bynk.generics.unknown_where_param",
        blurb: "A `where` clause bounds a type parameter, so it must name one: the function's own, or, for a method, its receiver type's. A name that is neither is usually a misspelling. Correct it, or declare the parameter in the brackets after the function's name. A bound that does not exist is `bynk.generics.unknown_constraint`.",
        example: "fn same[T](a: T) -> T where U: Eq { a }   // ✗ `U` is not a parameter here\n\nfn same[T](a: T) -> T where T: Eq { a }   // ✓ bound a parameter that is declared",
        page: "/book/reference/generics",
        anchor: "bounds",
    },
    Explain {
        code: "bynk.given.cross_context_unknown_capability",
        blurb: "A context may only depend on a capability that the context providing it exports. `exports capability { … }` is the provider's decision about what it offers other contexts, and a capability it leaves out stays its own. So `consumes clock { Clock }`, or a qualified `given clock.Clock`, is refused when `clock` does not list `Clock`. Add the name to the provider's `exports capability` clause (it must declare and provide the capability), or correct the spelling here. A context that is not consumed at all is `bynk.resolve.unconsumed_context`.",
        example: "// shop.bynk\ncontext shop\n\nconsumes clock { Clock }   // ✗ `clock` exports no `Clock`\n\n\
                  // clock.bynk\ncontext clock\n\nexports capability { Clock }   // ✓ the provider offers it",
        page: "/book/reference/capabilities",
        anchor: "",
    },
    Explain {
        code: "bynk.given.fn_outside_context",
        blurb: "`given` lets a function use capabilities, and only a context declares which capabilities it consumes. A commons is shared by every context that uses it, and those may consume different capabilities, so a function in a commons or a suite cannot declare `given`. Take the operation as a parameter instead, and pass `(m) => Logger.error(m)` from the handler, or move the function into the context that consumes the capability. A method cannot declare `given` anywhere: that is `bynk.given.fn_unsupported`.",
        example: "fn warn(what: String) -> Effect[()] given Logger { … }   // ✗ a commons function has no `given`\n\nfn warn(log: (String) -> Effect[()], what: String) -> Effect[()] { … }   // ✓ the caller passes the operation in",
        page: "/book/reference/capabilities",
        anchor: "capabilities-in-functions",
    },
    Explain {
        code: "bynk.given.fn_unsupported",
        blurb: "Only a free function in a context that returns `Effect[T]` may declare `given`. A capability operation is an effect, so a function that reaches one has to say so in its return type, and a method has no place to carry the capabilities. `Events` is refused too: `Events.emit` buffers until its handler commits, so emit in the handler, or pass the event back to the caller. Return `Effect[T]` and make it a free function, or take the operation as a function-value parameter. A `given` in a commons is `bynk.given.fn_outside_context`.",
        example: "fn size(sku: String) -> Int given Logger {   // ✗ it must return an `Effect`\n    3\n}\n\nfn size(sku: String) -> Effect[Int] given Logger {   // ✓\n    Effect.pure(3)\n}",
        page: "/book/reference/capabilities",
        anchor: "capabilities-in-functions",
    },
    Explain {
        code: "bynk.given.missing_for_call",
        blurb: "A free function in a context may declare `given`, and then uses those capabilities as a handler \
                does. Because a `given` clause is the honest statement of everything a body can reach, a \
                caller has to account for what its callee reaches: a call to, or a reference to, such a \
                function must occur in a body whose own `given` includes every one of the callee's. \
                Otherwise a handler's signature would hide capabilities it uses in effect. The rule is \
                checked at every call, so it holds along a whole chain of calls, and the call counts as \
                a use of the callee's capabilities. Add the missing capabilities to the caller's \
                `given`; the error names which ones. Using a capability directly, without declaring \
                it, is `bynk.given.undeclared_capability`.",
        example: "fn warn(what: String) -> Effect[()] given Logger { … }\n\n\
                  on call(sku: String) -> Effect[()] {\n    \
                    warn(sku)                           // ✗ `warn` needs `given Logger`\n}\n\n\
                  on call(sku: String) -> Effect[()] given Logger {   // ✓ declared on the caller\n    \
                    warn(sku)\n}",
        page: "/book/reference/capabilities",
        anchor: "capabilities-in-functions",
    },
    Explain {
        code: "bynk.given.undeclared_capability",
        blurb: "A handler may only use a capability it has itself declared with \
                `given`. Effects in Bynk are explicit: the `given` clause is the \
                handler's honest, checkable statement of every capability it \
                reaches for, so a reader (and the compiler) can see a handler's \
                full reach from its signature alone. Using a capability that is \
                not in `given` is the missing half of that contract.",
        example: "on call() -> Effect[()] {\n    \
                    Logger.log(\"hi\")                     // ✗ uses Logger without declaring it\n}\n\n\
                  on call() -> Effect[()] given Logger {   // ✓ declared, then used\n    \
                    Logger.log(\"hi\")\n}",
        page: "/book/guides/effects-and-capabilities/understand-the-capability-model",
        anchor: "",
    },
    Explain {
        code: "bynk.given.unknown_capability",
        blurb: "A `given` clause names a capability that no provider declares. A \
                capability is a typed interface to the outside world; it has to be \
                *declared* (as a `capability`, or brought in from a consumed \
                context) before a handler can ask for it. This usually means a \
                typo in the capability name, or a missing `uses`/`consumes` that \
                would bring the capability into scope.",
        example: "provides Greeter = Polite given Loger { … }    // ✗ no capability named `Loger`\n\n\
                  provides Greeter = Polite given Logger { … }   // ✓ matches the declared capability",
        page: "/book/reference/capabilities",
        anchor: "declaring-a-capability",
    },
    Explain {
        code: "bynk.held.branch_divergence",
        blurb: "A held value, such as a `Connection[F]`, must be disposed of exactly once. If one branch of an `if` or `match` closes, stores or passes it on while another leaves it, the compiler cannot say afterwards whether the value is still open to close, or already gone. Make every branch dispose of it, or none, and dispose of it once after the conditional. A connection that is never disposed of is `bynk.held.leak`.",
        example: "let _ <- if m == \"a\" { conn.close() } else { conn.send(ServerFrame { text: \"keep\" }) }   // ✗ one branch closes it\n\nlet _ <- if m == \"a\" { conn.send(ServerFrame { text: \"bye\" }) } else { conn.send(ServerFrame { text: \"keep\" }) }   // ✓ neither does\nlet _ <- conn.close()                                                                                          // ✓ closed once, after",
        page: "/book/reference/websocket",
        anchor: "sending-and-broadcasting",
    },
    Explain {
        code: "bynk.held.consume_on_borrow",
        blurb: "A held value such as a `Connection[F]` has exactly one owner, and the owner must \
                dispose of it by storing, closing or handing it on. A closure that visits each \
                stored connection (`forEach`, `parTraverse` and their variants) is only lent \
                each one: it may `send`, which leaves the connection owned, but `close` or any \
                transfer would be an owner's act performed on a value the closure does not own. A \
                match guard borrows in the same \
                way, because it runs before its arm is chosen. To close connections, remove \
                them from the store, which closes them, in the owner's code and not in the \
                closure.",
        example: "conns.forEach((c: Connection[ServerFrame]) => c.close())                          // ✗ `c` is borrowed\n\n\
                  conns.forEach((c: Connection[ServerFrame]) => c.send(ServerFrame { text: \"closing\" }))   // ✓ `send` does not consume",
        page: "/book/reference/websocket",
        anchor: "sending-and-broadcasting",
    },
    Explain {
        code: "bynk.held.leak",
        blurb: "A held value such as a `Connection[F]` has one owner, who must dispose of it before its scope ends: close it, store it (in a `Map[K, Connection[F]]` or a `Cell[Option[Connection[F]]]`), or pass it to a function or agent call that takes it. A dropped connection would stay open with nothing able to close it. The error is reported at the binding: a handler or function parameter, or a `let`. The connection `on open` receives is owned; the one `on message` and `on close` receive is only lent, so it carries no such duty. See `bynk.held.use_after_consume` (used after disposal) and `bynk.held.consume_on_borrow` (disposed of while borrowed).",
        example: "fn ignore(c: Connection[ServerFrame]) -> Effect[()] {   // ✗ `c` is never closed, stored or passed on\n  Effect.pure(())\n}\n\nfn finish(c: Connection[ServerFrame]) -> Effect[()] {   // ✓\n  c.close()\n}",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
    Explain {
        code: "bynk.held.query_accessor_on_held_map",
        blurb: "A held `Map[K, Connection]` is iterated with the broadcast operations, `forEach` (one at a time) or `parTraverse` (in parallel), not with a key query. `.entries`, `.keys` and `.values` would lift the connections out into a query, which the single-owner rules for held values do not allow. To send to every connection, broadcast over the map; to learn who is connected, keep a separate `Set` of their keys.",
        example: "conns.keys.collect()   // ✗ a held map has no key queries\n\nlet _ <- conns.parTraverse((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))   // ✓ broadcast",
        page: "/book/reference/websocket",
        anchor: "sending-and-broadcasting",
    },
    Explain {
        code: "bynk.held.unsupported_map_op",
        blurb: "`update` and `upsert` rebuild a map entry from its old value with a function. A held connection cannot be rebuilt from another: it is a live handle, stored and found by identity. So a held `Map[K, Connection]` takes `put`, `get`, `remove`, `contains` and `size`, and not these two. To replace an entry, `put` the new connection under the key, and to drop one, `remove` it.",
        example: "let _ <- conns.update(u, (c: Connection[ServerFrame]) => c)   // ✗ a connection cannot be derived from another\n\nlet _ <- conns.put(u, c)   // ✓ store the connection itself",
        page: "/book/reference/websocket",
        anchor: "sending-and-broadcasting",
    },
    Explain {
        code: "bynk.held.unsupported_storage",
        blurb: "A held value, a `Connection[F]`, is a live handle owned by one place, not data. Only `Cell[Option[Connection]]` and `Map[K, Connection]` can hold one: a `Set` needs value equality, which a connection does not have, and a `Log` or `Cache` could retain or evict one without closing it. Keep connections in a `Map` keyed by something that identifies each, such as a user id, and iterate it with the broadcast operations.",
        example: "store conns: Set[Connection[ServerFrame]]   // ✗ a `Set` cannot hold a connection\n\nstore conns: Map[UserId, Connection[ServerFrame]]   // ✓ held by key",
        page: "/book/reference/types",
        anchor: "connection",
    },
    Explain {
        code: "bynk.held.use_after_consume",
        blurb: "Closing, storing or passing on a held value such as a `Connection[F]` hands its single ownership away, and after that the name no longer refers to anything you may use. A later `send`, a second `close`, or any other mention of the binding is an error, reported on that later use, because the connection may already be closed or owned by someone else. Only `send` leaves a connection owned, so do every `send` first and dispose of the connection last. `bynk.held.leak` is the other end of the same rule (never disposed), and `bynk.held.consume_on_borrow` is the case where the code never owned the connection to begin with.",
        example: "let _ <- c.close()\nlet _ <- c.send(ServerFrame { text: \"bye\" })   // ✗ `c` was closed on the line above\n\nlet _ <- c.send(ServerFrame { text: \"bye\" })   // ✓ send first\nlet _ <- c.close()",
        page: "/book/reference/websocket",
        anchor: "sending-and-broadcasting",
    },
    Explain {
        code: "bynk.history.not_an_agent",
        blurb: "`for all run: History[T]` generates a run of calls to an agent's handlers and drives them against a fresh instance. Only an agent has handlers to sequence, so `T` must be one declared in scope. For a value, a plain `for all x: T` generates the value. Name the agent.",
        example: "// tests/t.test.bynk\nfor all run: History[Amount] { … }   // ✗ `Amount` is a value type\nfor all run: History[Wallet] { … }   // ✓ an agent",
        page: "/book/reference/testing",
        anchor: "history-properties",
    },
    Explain {
        code: "bynk.history.not_generable",
        blurb: "To drive an agent the runner must make an argument for every handler, drawing each from its parameter's refinement domain. One parameter with no generator, such as a `Matches` pattern, stops it for the whole agent, since any handler may be chosen. Give that parameter a refinement that has one, such as `Length(3)`. The same rule applies to a value `for all`, where a type that cannot be generated is `bynk.val.needs_pin`.",
        example: "// demo/wallet.bynk\ntype Code = String where Matches(\"[A-Z]{3}\")\n\n// tests/t.test.bynk\nfor all run: History[Wallet] { … }   // ✗ `setLabel` takes a `Code`, which has no generator\n\n// demo/wallet.bynk\ntype Code = String where Length(3)   // ✓ a refinement with a generator",
        page: "/book/reference/testing",
        anchor: "history-properties",
    },
    Explain {
        code: "bynk.history.outside_property",
        blurb: "`History[Agent]` is a generator, not a value type: it stands for a run the test runner makes for a `for all` binding. So it may appear only as the type of that binding inside a `property`, never in a function signature, a field or a `let`. Move the check into a `property` and assert over `run`.",
        example: "// demo/wallet.bynk\nfn peek(w: History[Wallet]) -> Int { 0 }   // ✗ not a value type\n\n// tests/t.test.bynk\nfor all run: History[Wallet] { … }         // ✓ in a `property`",
        page: "/book/reference/testing",
        anchor: "history-properties",
    },
    Explain {
        code: "bynk.history.restates_invariant",
        blurb: "Every state a driven run reaches has already passed the agent's `invariant` and `transition` checks, so a history property that only asserts the same thing over `run` cannot fail. The compiler flags it, and the check is deliberately narrow, so a near-copy may pass. Assert something the per-state rules do not say, such as a protocol across steps: a spend is accepted only after a top-up.",
        example: "// demo/wallet.bynk\ninvariant nonneg: balance >= 0\n\n// tests/t.test.bynk\nfor all run: History[Wallet] { expect run.all((s) => s.new.balance >= 0) }   // ✗ the invariant says this\nfor all run: History[Wallet] { expect run.all((s) => (s.call is Spend && s.accepted) implies run.upTo(s).any((p) => p.call is TopUp && p.accepted)) }   // ✓ a protocol across steps",
        page: "/book/reference/testing",
        anchor: "history-properties",
    },
    Explain {
        code: "bynk.http.body_kind_conflict",
        blurb: "`@raw`, `@form` and `@multipart` each say how a route's one `body` is read off the wire: untouched octets, a urlencoded form or a multipart upload. A body has one wire format, so a handler carries at most one of them, and the second one written is reported. Keep the annotation that matches what the client sends; a JSON body needs none of them.",
        example: "@form\n@raw(contentType: \"image/png\")   // ✗ the body is already a form\n@limit(maxBody: 1_000_000)\non POST(\"/signup\") (body: SignupForm) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@form                           // ✓ one wire format\non POST(\"/signup\") (body: SignupForm) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "raw-request-bodies",
    },
    Explain {
        code: "bynk.http.body_on_get_or_delete",
        blurb: "A `GET` or `DELETE` request carries no body by convention, and proxies and some clients drop one, so a handler for either may not declare a `body` parameter. Send the data in the path or the query string (`query: Filter` decodes a record from it), or make the route a `POST`, `PUT` or `PATCH` if the client has something to upload. `@raw` and `@form` need a body too: `bynk.http.raw_on_bodyless`, `bynk.http.form_on_bodyless`.",
        example: "type Filter = { tag: String }\n\non GET(\"/items\") (body: Filter) -> Effect[HttpResult[String]] by v: Visitor { … }    // ✗ a GET has no body\n\non GET(\"/items\") (query: Filter) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓ read it from the query string",
        page: "/book/reference/http",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.http.cache_bad_immutable",
        blurb: "`immutable` on `@cache` promises that the response at this URL never changes, so a cache may reuse it for the whole `maxAge` without asking again. A promise is either made or not, so the value must be the literal `true` or `false`; a bare word, a string or an expression is refused. Write `immutable: true` for a content-addressed path, or leave the argument out. The other arguments are `bynk.http.cache_bad_max_age` and `bynk.http.cache_bad_scope`.",
        example: "@cache(maxAge: 365.days, immutable: yes)    // ✗ `true` or `false`\non GET(\"/dl/:hex\") (hex: String) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@cache(maxAge: 365.days, immutable: true)   // ✓\non GET(\"/dl/:hex\") (hex: String) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "caching",
    },
    Explain {
        code: "bynk.http.cache_bad_max_age",
        blurb: "`@cache` says how long a client or cache may reuse a `GET` response without asking again, so it needs a `maxAge`, and that must be a positive `Duration` literal. A bare number, a string or a singular unit such as `1.hour` is refused; the units are plural: `30.seconds`, `5.minutes`, `1.hours`. A positive duration that is not a whole number of seconds is `bynk.http.cache_max_age_fractional_seconds`.",
        example: "@cache(maxAge: 1.hour)    // ✗ the unit is plural\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@cache(maxAge: 1.hours)   // ✓\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\nconst Fresh: Duration = 1.hours\n@cache(maxAge: Fresh)   // ✓ a `Duration` constant bound to a literal\non GET(\"/docs\") () -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "caching",
    },
    Explain {
        code: "bynk.http.cache_bad_scope",
        blurb: "`scope` says who may store a cached response: `private` means the user's own browser only, and `public` also lets a shared cache, such as a CDN, keep it. Those are the only two values, written bare rather than as strings, so `shared` or `\"public\"` is refused. Choose one, or omit `scope` for `private`. Use `public` only for a response that is the same for everyone: `bynk.http.cache_public_reads_cookies`.",
        example: "@cache(maxAge: 5.minutes, scope: shared)   // ✗ `public` or `private`\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@cache(maxAge: 5.minutes, scope: public)   // ✓",
        page: "/book/reference/http",
        anchor: "caching",
    },
    Explain {
        code: "bynk.http.cache_duplicate",
        blurb: "A handler has one `@cache`, which says how long the response to that route may be reused. Two would give it two freshness windows, and neither could be said to win, so the second is refused. Delete the one you do not want. A handler may likewise carry only one `@limit` (`bynk.http.limit_duplicate`), `@raw` or `@json`.",
        example: "@cache(maxAge: 1.minutes)\n@cache(maxAge: 5.minutes)   // ✗ already cached above\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@cache(maxAge: 5.minutes)   // ✓ one window\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "caching",
    },
    Explain {
        code: "bynk.http.cache_max_age_fractional_seconds",
        blurb: "`Cache-Control: max-age` counts whole seconds. A `maxAge` such as `1500.milliseconds` has a remainder that would be dropped, and the header would say `1` for what you wrote as 1.5, so it is refused rather than quietly truncated. Write a whole number of seconds, rounding it yourself. A `maxAge` that is not a positive duration at all is `bynk.http.cache_bad_max_age`.",
        example: "@cache(maxAge: 1500.milliseconds)   // ✗ not a whole number of seconds\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@cache(maxAge: 2.seconds)           // ✓",
        page: "/book/reference/http",
        anchor: "caching",
    },
    Explain {
        code: "bynk.http.cache_on_non_get",
        blurb: "`@cache` declares how long the response to a read may be reused, so it is legal only on an `on GET` handler. Conditional caching applies to safe, idempotent reads, and a `POST`, `PUT`, `PATCH` or `DELETE` is neither. Remove the annotation, or, if the route only reads, make it a `GET`. A `GET` is revalidated by its `ETag` with no annotation at all.",
        example: "@cache(maxAge: 5.minutes)   // ✗ a `POST` changes something\non POST(\"/refresh\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@cache(maxAge: 5.minutes)   // ✓ a read\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "caching",
    },
    Explain {
        code: "bynk.http.cache_public_reads_cookies",
        blurb: "`scope: public` lets a shared cache, such as a CDN, store the response and serve it to anyone who asks for the URL. A handler that reads `cookies` answers for whoever is asking, so a shared cache could replay one user's response to another. Use `scope: private`, the default, which only the user's own browser may keep, or drop the `cookies` parameter. A response that sets a cookie is always sent as private.",
        example: "type Session = { sid: Option[String] }\n\n@cache(maxAge: 5.minutes, scope: public)    // ✗ the answer depends on who asks\non GET(\"/me\") (cookies: Session) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@cache(maxAge: 5.minutes, scope: private)   // ✓",
        page: "/book/reference/http",
        anchor: "caching",
    },
    Explain {
        code: "bynk.http.cache_unknown_arg",
        blurb: "`@cache` takes three named arguments: `maxAge`, `scope` and `immutable`. The set is closed, so a misspelt or invented option is refused rather than ignored, which would leave the route with caching you did not ask for. A positional argument is refused too. Rename the argument or remove it. The values are checked by `bynk.http.cache_bad_max_age`, `bynk.http.cache_bad_scope` and `bynk.http.cache_bad_immutable`.",
        example: "@cache(maxAge: 5.minutes, stale: true)   // ✗ not an argument of `@cache`\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@cache(maxAge: 5.minutes)                // ✓\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "caching",
    },
    Explain {
        code: "bynk.http.cookie_cross_site_without_credentials",
        blurb: "A cookie with `SameSite=None` (`SameSite.Unrestricted`) exists to travel on cross-site requests, and a browser attaches cookies to a cross-origin `fetch` only when the request is credentialed and the response carries `Access-Control-Allow-Credentials: true`. A service with no `cors` policy, or one with `credentials: false`, never sends that header, so a cross-origin `fetch` can never send the cookie back. This is a warning, because a `SameSite=None` cookie is still sent on top-level cross-site navigations, cross-site form posts and in embeds (SSO and payment callbacks, widgets), none of which needs CORS. If a page on another origin must send it back with `fetch`, declare `cors { origins: [...], credentials: true }` (the origins must be exact, not `[\"*\"]`), or use `SameSite.Lax` if the cookie need not cross sites at all.",
        example: "service api from http {\n  on POST(\"/login\") () -> Effect[HttpResult[String]] by v: Visitor {\n    Created(\"ok\").withCookie(Cookie { name: \"sid\", value: \"a\", sameSite: SameSite.Unrestricted })   // ⚠ no `cors { credentials: true }`\n  }\n}\n\nservice api from http {\n  cors { origins: [\"https://app.example.com\"], credentials: true }   // ✓\n  …\n}",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_domain_widens",
        blurb: "A cookie with no `Domain` attribute belongs to exactly the host that set it. Setting `domain` makes it available to that domain and every subdomain, so a sibling service, or a subdomain an attacker can take over, receives it and can set a cookie of the same name. That is rarely wanted, so it is a warning. Drop `domain`; or, if the cookie is a session cookie, name it `__Host-…`, which makes the compiler refuse a `domain` altogether (`bynk.http.cookie_host_prefix`). Keep `domain` only when subdomains genuinely share the login.",
        example: "Created(\"ok\").withCookie(Cookie { name: \"sid\", value: \"a\", domain: Some(\"example.com\") })   // ⚠ every subdomain receives it\n\nCreated(\"ok\").withCookie(Cookie { name: \"__Host-sid\", value: \"a\" })                           // ✓ this host only",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_field_arity",
        blurb: "`Record.field.cookie(value)` writes a typed cookie from a record's field, so it takes exactly the value to write, and `Record.field.expire()` takes nothing: the field is named by the path, not by an argument. Write `Prefs.theme.cookie(Dark)` to set it and `Prefs.theme.expire()` to remove it.",
        example: "Ok(\"x\").withCookie(Prefs.theme.cookie())       // ✗ the value to write is missing\n\nOk(\"x\").withCookie(Prefs.theme.cookie(Dark))   // ✓",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_field_list",
        blurb: "A `List` field of a `cookies` record is a cookie name sent several times, so writing one would be several `Set-Cookie` lines, which one `Cookie` cannot be. The typed write is refused for it. Write each cookie with an untyped `Cookie { name, value }` and `withCookie`.",
        example: "Ok(\"x\").withCookie(Prefs.tags.cookie(\"a\"))   // ✗ `tags` is a `List` field\n\nOk(\"x\").withCookie(Prefs.theme.cookie(Dark))   // ✓ a scalar field",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_field_not_cookie_record",
        blurb: "A typed cookie write is named and encoded as its record's decoder reads it, so the record must be the `cookies` parameter of an `on http` handler in this unit: that decoder is what the write agrees with. For a cookie in no record, write an untyped `Cookie { name, value }`.",
        example: "type Other = { theme: Theme = Light }\nOk(\"x\").withCookie(Other.theme.cookie(Dark))   // ✗ no handler decodes `Other` as `cookies`\n\nOk(\"x\").withCookie(Prefs.theme.cookie(Dark))    // ✓ `Prefs` is a handler's `cookies` record",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_field_unknown",
        blurb: "`Record.field.cookie(value)` and `Record.field.expire()` name a field of the cookie record, and the field is the one the cookie is named and encoded by. The name given is not a field of the record; the note lists its fields.",
        example: "Ok(\"x\").withCookie(Prefs.mode.cookie(Dark))    // ✗ `Prefs` has no field `mode`\n\nOk(\"x\").withCookie(Prefs.theme.cookie(Dark))   // ✓",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_host_prefix",
        blurb: "A cookie named `__Host-…` asks the browser to keep it only if it is `Secure`, has no `Domain` and has `Path=/`. That ties it to this exact host, so a sibling subdomain or an insecure page cannot set or overwrite it. A browser drops one that breaks any of the three, so a literal that does is refused. Remove `secure: false`, the `domain` and any `path` other than `\"/\"`, or rename the cookie.",
        example: "Created(\"ok\").withCookie(Cookie { name: \"__Host-sid\", value: \"a\", path: \"/app\" })   // ✗ must be `/`\n\nCreated(\"ok\").withCookie(Cookie { name: \"__Host-sid\", value: \"a\" })                 // ✓",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_partitioned_insecure",
        blurb: "A `partitioned` cookie is stored separately for each top-level site that embeds your service, and browsers accept the `Partitioned` attribute only with `Secure`. A literal that sets `partitioned: true` with `secure: false` would be dropped, so it is refused. Remove `secure: false` (the default is `secure: true`), or drop `partitioned`. See also `bynk.http.cookie_samesite_none_insecure`.",
        example: "Created(\"ok\").withCookie(Cookie { name: \"sid\", value: \"a\", partitioned: true, secure: false })   // ✗\n\nCreated(\"ok\").withCookie(Cookie { name: \"sid\", value: \"a\", partitioned: true })                  // ✓",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_samesite_none_insecure",
        blurb: "`SameSite=None` lets a cookie travel on cross-site requests, and browsers accept it only with `Secure`, so it is sent over HTTPS alone. Without it the browser drops the cookie and the login or session quietly fails, so Bynk refuses the literal. Remove `secure: false`, since the default is `secure: true`, or choose `Lax` or `Strict` if the cookie need not cross sites.",
        example: "Created(\"ok\").withCookie(Cookie { name: \"sid\", value: \"a\", sameSite: SameSite.Unrestricted, secure: false })   // ✗\n\nCreated(\"ok\").withCookie(Cookie { name: \"sid\", value: \"a\", sameSite: SameSite.Unrestricted })                  // ✓",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_secure_prefix",
        blurb: "A cookie named `__Secure-…` asks the browser to accept it only over HTTPS, and a browser drops one that is not `Secure`, so a literal with `secure: false` would never arrive. It is refused instead. Remove `secure: false`; the default is `secure: true`. A `__Host-` name is stricter: `bynk.http.cookie_host_prefix`.",
        example: "Created(\"ok\").withCookie(Cookie { name: \"__Secure-sid\", value: \"a\", secure: false })   // ✗\n\nCreated(\"ok\").withCookie(Cookie { name: \"__Secure-sid\", value: \"a\" })                  // ✓",
        page: "/book/reference/http",
        anchor: "setting-cookies",
    },
    Explain {
        code: "bynk.http.cookie_untyped_has_field",
        blurb: "`Cookie { name: \"theme\", value: \"Dark\" }` is a cookie in no record, but here the record `Prefs` reads that cookie by that name, so the name and the value are written twice with nothing relating them: a renamed field or a changed type leaves the write valid and wrong. It is a warning. Write it from the record, `Prefs.theme.cookie(Dark)`, which is named by the field's wire name and valued as its decoder reads it; keep the attributes by spreading it into a `Cookie` literal. Where the literal is only a name and a value that provably decodes, the quick-fix does the rewrite.",
        example: "Ok(\"x\").withCookie(Cookie { name: \"theme\", value: \"Dark\" })   // ⚠ `Prefs` reads this cookie\n\nOk(\"x\").withCookie(Prefs.theme.cookie(Dark))                       // ✓",
        page: "/book/reference/http",
        anchor: "typed-cookie-write",
    },
    Explain {
        code: "bynk.http.cookie_value_undecodable",
        blurb: "An untyped `Cookie` literal is named for a field of a `cookies` record, so the next request is read by that field's decoder, and its literal value is one the decoder rejects (an enum is read by its exact, case-sensitive variant name, an `Int` by its digits, a `Bool` by `true` or `false`). The write compiles and is sent, and then every later request is answered `400`. It is an error. Write the cookie from the record, `Prefs.theme.cookie(Dark)`, which checks the value against the field.",
        example: "Ok(\"x\").withCookie(Cookie { name: \"mode\", value: \"dark\" })   // ✗ `Mode` is read as `Light` or `Dark`\n\nOk(\"x\").withCookie(Cookie { name: \"mode\", value: \"Dark\" })   // ✓",
        page: "/book/reference/http",
        anchor: "typed-cookie-write",
    },
    Explain {
        code: "bynk.http.cookies_field_not_stringy",
        blurb: "A cookie is a single text value, so each field of the `cookies` record must be one a single value can fill: a `String`, `Int`, `Bool` or `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` (a cookie that may be absent) or a `List` (a name sent twice). A nested record or any other type is refused. Flatten the record, or keep structured data in a `String` and decode it yourself. `query` and `@form` records follow the same rule: `bynk.http.query_field_not_stringy`, `bynk.http.form_field_not_stringy`.",
        example: "type Prefs = { dark: Bool }\n\ntype Session = { prefs: Prefs }   // ✗ a cookie is one value, not a record\n\ntype Session = { dark: Bool }     // ✓\n\non GET(\"/me\") (cookies: Session) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "cookies",
    },
    Explain {
        code: "bynk.http.cookies_not_http",
        blurb: "`RequestCookies` is the cookies of an HTTP request, read from its `Cookie` header. A `cron` or other handler that is not an `on http` route answers no request, so there are no cookies for it to take, and the parameter is refused. Remove it, or move the work into an HTTP handler. On an HTTP route, `cookies` is a record or `RequestCookies` and nothing else: `bynk.http.cookies_wrong_type`. `RequestQuery` has the same rule: `bynk.http.query_not_http`.",
        example: "service ticker from cron {\n    on schedule(\"*/5 * * * *\") (cookies: RequestCookies) -> Effect[Result[(), String]] { … }   // ✗ no request\n}\n\ntype Session = { sid: Option[String] }\n\nservice ticker from http {\n    on GET(\"/me\") (cookies: Session) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓\n}",
        page: "/book/reference/http",
        anchor: "request-cookies",
    },
    Explain {
        code: "bynk.http.cookies_wrong_type",
        blurb: "`cookies` is the name the router reserves for the request's cookies. It decodes the `Cookie` header into the record you name, one field per cookie, and answers `400` when they do not fit, so the parameter must be a record type. Declare the cookies you read as a record, with an `Option` for each that may be absent. A field a cookie cannot carry is `bynk.http.cookies_field_not_stringy`.",
        example: "type Session = { sid: Option[String] }\n\non GET(\"/me\") (cookies: String) -> Effect[HttpResult[String]] by v: Visitor { … }    // ✗ not a record\n\non GET(\"/me\") (cookies: Session) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "cookies",
    },
    Explain {
        code: "bynk.http.cors_invalid_field",
        blurb: "Each optional `cors` field takes one kind of value: `headers` a list of string literals, `credentials` the literal `true` or `false`, and `maxAge` a `Duration` literal such as `1.hours`. They are written straight into the `Access-Control-*` headers the router sends, so they are fixed at compile time: a string, a number or a computed value is refused. `origins`, the required field, is checked by `bynk.http.cors_invalid_origins`.",
        example: "cors { origins: [\"https://app.example.com\"], credentials: \"yes\" }   // ✗ `true` or `false`\n\ncors { origins: [\"https://app.example.com\"], credentials: true }    // ✓",
        page: "/book/reference/http",
        anchor: "cors",
    },
    Explain {
        code: "bynk.http.cors_invalid_origins",
        blurb: "`origins` is the allowlist the router checks each request's `Origin` against, so it is required and must be a non-empty list of string literals: exact origins such as `\"https://app.example.com\"`, or `[\"*\"]` for any site. A single string, an empty list, a computed entry or no `origins` at all leaves nothing to compare, and is refused. The wildcard cannot be combined with `credentials: true`: `bynk.http.cors_wildcard_credentials`.",
        example: "cors { origins: \"https://app.example.com\" }     // ✗ a list of strings\n\ncors { origins: [\"https://app.example.com\"] }   // ✓",
        page: "/book/reference/http",
        anchor: "cors",
    },
    Explain {
        code: "bynk.http.cors_not_http",
        blurb: "A `cors { }` block says which other sites a browser may let call the service, and only the HTTP surface has cross-origin browser callers, so it is valid only on a `from http` service. On a `cron` or other service it has no request to apply to. Move it to the `from http` service it was meant for, or delete it. The `security`, `limits` and `json` blocks follow the same rule: `bynk.http.security_not_http`, `bynk.http.limits_not_http`, `bynk.http.json_not_http`.",
        example: "service jobs from cron {\n    cors { origins: [\"https://app.example.com\"] }   // ✗ a schedule has no browser callers\n    on schedule(\"0 0 * * *\") () -> Effect[Result[(), String]] { … }\n}\n\nservice api from http {\n    cors { origins: [\"https://app.example.com\"] }   // ✓\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "cors",
    },
    Explain {
        code: "bynk.http.cors_unknown_field",
        blurb: "A `cors { }` policy has four fields: `origins`, `headers`, `credentials` and `maxAge`. The set is closed, so a misspelt or invented field is refused rather than ignored, which would leave the policy different from the one you wrote. `Access-Control-Allow-Methods` is not a field: the compiler derives it from the service's routes. Rename the field or delete it. A field with the wrong kind of value is `bynk.http.cors_invalid_field`.",
        example: "cors { origins: [\"https://app.example.com\"], methods: [\"GET\"] }   // ✗ not a field, methods come from the routes\n\ncors { origins: [\"https://app.example.com\"], maxAge: 1.hours }    // ✓",
        page: "/book/reference/http",
        anchor: "cors",
    },
    Explain {
        code: "bynk.http.cors_wildcard_credentials",
        blurb: "`credentials: true` lets a browser send cookies and `Authorization` to the service, and the Fetch standard forbids answering such a request with the wildcard origin `*`: the browser rejects the response at runtime. Bynk refuses the pair at compile time instead. List the origins that may send credentials, or set `credentials: false` if any site may call the service.",
        example: "cors { origins: [\"*\"], credentials: true }                        // ✗ credentials need named origins\n\ncors { origins: [\"https://app.example.com\"], credentials: true }   // ✓",
        page: "/book/reference/http",
        anchor: "cors",
    },
    Explain {
        code: "bynk.http.duplicate_rejected",
        blurb: "`on rejected` renders every refusal the router makes for a service's routes, so a service has one and it must handle every case: a second would leave the router choosing between them. Merge the two into one that matches on `why`. Its required shape is `bynk.http.rejected_signature`.",
        example: "on rejected(why: BoundaryRejection) -> Effect[HttpResult[()]] { … }\non rejected(why: BoundaryRejection) -> Effect[HttpResult[()]] { … }   // ✗ already declared above\n\non rejected(why: BoundaryRejection) -> Effect[HttpResult[()]] {\n    match why {\n        MethodNotAllowed => RawError(MethodNotAllowed, Bytes.empty(), \"text/plain\")   // ✓ one hook, all cases\n        _ => RawError(BadRequest, Bytes.empty(), \"text/plain\")\n    }\n}",
        page: "/book/reference/http",
        anchor: "rendering-rejections",
    },
    Explain {
        code: "bynk.http.duplicate_route",
        blurb: "A route is a method and a path, and the router needs exactly one handler to answer it. Two handlers that declare the same pair, in one service or in two services of a context, leave it nothing to choose between, so the later one is refused. Merge them, or give one a different path or method.",
        example: "on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\non GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }        // ✗ already declared above\n\non GET(\"/items/:id\") (id: String) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓ a different path",
        page: "/book/reference/http",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.http.error_status_position",
        blurb: "An `ErrorStatus` name such as `Forbidden` stands for a status only as \
                `RawError`'s first argument, where the compiler turns it into the \
                number it answers. It is not a type, so a status can't be stored or \
                passed to a helper. Name the status where the response is built, and \
                let a helper build the body.",
        example: "let status = ErrorStatus.Forbidden                          // ✗ a status has no value here\n\n\
                  fn problem(msg: String) -> Bytes { Bytes.fromUtf8(msg) }\n\
                  RawError(Forbidden, problem(\"no access\"), \"text/plain\")     // ✓",
        page: "/book/reference/http",
        anchor: "error-bodies",
    },
    Explain {
        code: "bynk.http.extra_param",
        blurb: "A handler's parameters are not free-form. Each is a path parameter, matching a `:name` in the route, or one of the three the router fills itself: `body`, `headers` and `query`. Any other name has nowhere to come from. Read a value from the query string with `query: RequestQuery` or a record, move it into the path, or drop it. A `:name` with no parameter is `bynk.http.unbound_path_param`.",
        example: "on GET(\"/items\") (page: String) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✗ not in the path, and not `body`, `headers` or `query`\n\non GET(\"/items\") (query: RequestQuery) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.http.form_body_not_record",
        blurb: "`@form` and `@multipart` decode a form body one key at a time, into the fields of a record, so the `body` must be a record type. A `String`, a list or any other type has no fields for the keys to fill. Declare a record with one field per form key and take it as `body`; each field is then held to `bynk.http.form_field_not_stringy`. A JSON body of another shape needs no annotation.",
        example: "type Signup = { email: String }\n\n@form\non POST(\"/signup\") (body: String) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✗ a form decodes into a record\n\n@form\non POST(\"/signup\") (body: Signup) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "form-bodies",
    },
    Explain {
        code: "bynk.http.form_duplicate",
        blurb: "`@form` says the route's one `body` is an HTML form post, and `@multipart` says it is a browser upload. A handler has a single body, so it carries one of the two, once: a second would restate the encoding or contradict it, and the second is refused. Delete the repeat. Pairing `@form` with `@raw` is `bynk.http.body_kind_conflict`, and a repeated `@raw` is `bynk.http.raw_duplicate`.",
        example: "type Signup = { email: String }\n\n@form\n@form    // ✗ a second `@form`\non POST(\"/signup\") (body: Signup) -> Effect[HttpResult[()]] by v: Visitor { … }\n\n@form    // ✓\non POST(\"/signup\") (body: Signup) -> Effect[HttpResult[()]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "form-bodies",
    },
    Explain {
        code: "bynk.http.form_field_not_stringy",
        blurb: "A form body is a flat set of keys with text values, so each field of a `@form` or `@multipart` body record must be one a single value can fill: a `String`, `Int`, `Bool` or `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` or a `List` (a repeated key). A multipart body may also have `File` fields. A nested record, or any other type, is refused; flatten it. A `query` record follows the same rule: `bynk.http.query_field_not_stringy`.",
        example: "type Address = { city: String }\ntype SignupForm = { email: String, address: Address }   // ✗ a form has no nesting\n\ntype SignupForm = { email: String, city: String }       // ✓ flat keys",
        page: "/book/reference/http",
        anchor: "form-bodies",
    },
    Explain {
        code: "bynk.http.form_on_bodyless",
        blurb: "`@form` and `@multipart` read a form post from the request's `body`, so they belong on a `POST`, `PUT` or `PATCH` route with a `body` parameter. On a `GET`, a `DELETE` or a route with no `body` there is nothing to decode. Add a `body` record, or drop the annotation. `@raw` has the same rule: `bynk.http.raw_on_bodyless`.",
        example: "type Signup = { email: String }\n\n@form   // ✗ the route takes no body\non POST(\"/signup\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@form\non POST(\"/signup\") (body: Signup) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "form-bodies",
    },
    Explain {
        code: "bynk.http.form_on_sum_route",
        blurb: "A route `by` a sum of actors, such as `User | Visitor`, reads and verifies the request body as JSON itself. `@form` and `@multipart` have the router decode the body in another format, so the two cannot both own it and the pair is refused. Name one actor on the route, or split it into one route per actor. A route with `@form` and a second wire format is `bynk.http.body_kind_conflict`.",
        example: "type UserId = opaque String\ntype Search = { q: String }\n\nactor User { auth = Bearer(secret = \"AUTH_JWT_SECRET\"), identity = UserId }\n\n@form\non POST(\"/search\") (body: Search) -> Effect[HttpResult[String]] by who: User | Visitor { … }   // ✗ a sum of actors\n\n@form\non POST(\"/search\") (body: Search) -> Effect[HttpResult[String]] by who: User { … }             // ✓ one actor",
        page: "/book/reference/http",
        anchor: "form-bodies",
    },
    Explain {
        code: "bynk.http.form_unknown_arg",
        blurb: "`@form` takes no arguments. A form body is always `application/x-www-form-urlencoded`, so there is no media type to name, and a size ceiling belongs to `@limit` or a `limits { }` section, not to the annotation. An argument is refused rather than ignored, which would leave the route behaving otherwise than you wrote. Write `@form` bare. `@multipart` takes none either. `@raw` takes `contentType:`: `bynk.http.raw_unknown_arg`.",
        example: "type Signup = { email: String }\n\n@form(contentType: \"application/x-www-form-urlencoded\")    // ✗ `@form` takes no argument\non POST(\"/signup\") (body: Signup) -> Effect[HttpResult[()]] by v: Visitor { … }\n\n@form    // ✓\non POST(\"/signup\") (body: Signup) -> Effect[HttpResult[()]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "form-bodies",
    },
    Explain {
        code: "bynk.http.headers_wrong_type",
        blurb: "`headers` is the name the router reserves for the request's headers, so a parameter called `headers` must be typed `RequestHeaders`, the read-only view with `get(name)`, `bearer()` and `clientIp()`. Any other type has nothing to be filled from. Retype the parameter. An input of your own needs another name, and a name that is neither a path parameter nor one of the reserved ones is `bynk.http.extra_param`.",
        example: "on GET(\"/whoami\") (headers: String) -> Effect[HttpResult[String]] by v: Visitor { … }           // ✗\n\non GET(\"/whoami\") (headers: RequestHeaders) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "request-headers",
    },
    Explain {
        code: "bynk.http.invalid_path",
        blurb: "The router matches a route against the request's path, which always begins with `/`, so a route that does not is refused rather than guessed at. A `:` segment must also carry a name, since the name is what binds the value to a handler parameter. Start the route with `/` and name each parameter, as in `\"/items/:id\"`. A route under a reserved prefix is `bynk.http.reserved_prefix`.",
        example: "on GET(\"items\") () -> Effect[HttpResult[String]] by v: Visitor { … }    // ✗ no leading `/`\n\non GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.http.json_duplicate",
        blurb: "`@json(options: …)` chooses how one route's response bodies write an `Option`. A handler has one, because two would give the same body two encodings with nothing to say which wins, so the second is refused. Keep the one you mean. To set the encoding for every route of a service, use a `json { }` section instead. A handler may likewise carry only one `@cache` (`bynk.http.cache_duplicate`), `@limit` or `@raw`.",
        example: "@json(options: plain)\n@json(options: tagged)    // ✗ a second `@json`\non GET(\"/activity\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@json(options: plain)     // ✓\non GET(\"/activity\") () -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "json-for-non-bynk-clients",
    },
    Explain {
        code: "bynk.http.json_invalid_arg",
        blurb: "`@json` has one argument, `options:`, whose value is the bare word `tagged` or `plain`: tagged writes `Option` as `{\"kind\": \"Some\", …}` (the default), and plain writes `Some(v)` as `v` and `None` as `null`. The annotation is refused with no `options`, or with any other value. Write `@json(options: plain)` or `@json(options: tagged)`. Another argument name is `bynk.http.json_unknown_arg`.",
        example: "@json(options: compact)   // ✗ `tagged` or `plain`\non GET(\"/activity\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@json(options: plain)     // ✓\non GET(\"/activity\") () -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "json-for-non-bynk-clients",
    },
    Explain {
        code: "bynk.http.json_invalid_field",
        blurb: "The `options` field of a `json { }` section says how response bodies write an `Option`, and it is read at compile time, so its value is the bare word `tagged` (the default, `{\"kind\": \"Some\", …}`) or `plain` (`Some(v)` as `v`, `None` as `null`). Any other word, a string or a computed value is refused. Write one of the two. The route-level form is `@json(options: …)`, checked by `bynk.http.json_invalid_arg`.",
        example: "service api from http {\n    json { options: compact }   // ✗ `tagged` or `plain`\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}\n\nservice api from http {\n    json { options: plain }     // ✓\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "json-for-non-bynk-clients",
    },
    Explain {
        code: "bynk.http.json_not_http",
        blurb: "A `json { }` section chooses how a service's HTTP response bodies write `Option`, tagged or plain. Only a `from http` service has response bodies, so on a `cron` or other service the section is refused. Move it to the HTTP service it was meant for, or delete it. The `cors`, `security` and `limits` sections have the same rule: `bynk.http.cors_not_http`, `bynk.http.security_not_http`, `bynk.http.limits_not_http`. For the per-route form, see `bynk.http.json_on_non_http`.",
        example: "service jobs from cron {\n    json { options: plain }   // ✗ a schedule has no response body\n    on schedule(\"0 0 * * *\") () -> Effect[Result[(), String]] { … }\n}\n\nservice api from http {\n    json { options: plain }   // ✓\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "json-for-non-bynk-clients",
    },
    Explain {
        code: "bynk.http.json_on_non_http",
        blurb: "`@json(options: …)` picks how one HTTP response body writes `Option`, tagged or plain. Only an `on http` handler answers with a response body, so the annotation is refused on a `schedule`, queue or other handler. Move it to an HTTP route, or remove it. The service-wide section, `json { }`, is limited to `from http` services too: `bynk.http.json_not_http`.",
        example: "service jobs from cron {\n    @json(options: plain)   // ✗ a schedule has no response body\n    on schedule(\"0 0 * * *\") () -> Effect[Result[(), String]] { … }\n}\n\nservice api from http {\n    @json(options: plain)   // ✓\n    on GET(\"/activity\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "json-for-non-bynk-clients",
    },
    Explain {
        code: "bynk.http.json_unknown_arg",
        blurb: "`@json` has one argument, `options:`. The set is closed, so a misspelt or invented argument is refused rather than ignored, which would leave the route writing `Option` otherwise than you asked. Rename it to `options` or remove it. The value is checked separately: `bynk.http.json_invalid_arg`. The same name in a service's `json { }` section is `bynk.http.json_unknown_field`.",
        example: "@json(options: plain, nulls: keep)    // ✗ not an argument of `@json`\non GET(\"/activity\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@json(options: plain)                 // ✓\non GET(\"/activity\") () -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "json-for-non-bynk-clients",
    },
    Explain {
        code: "bynk.http.json_unknown_field",
        blurb: "A `json { }` section sets how every route of a service writes an `Option` in a response body, and `options` is its only field. The set is closed, so a misspelt or invented field is refused rather than ignored, which would leave the service writing bodies otherwise than you wrote. Rename it to `options` or remove it. A bad value is `bynk.http.json_invalid_field`; the route-level `@json` form is `bynk.http.json_unknown_arg`.",
        example: "service api from http {\n    json { options: plain, nulls: keep }    // ✗ `options` is the only field\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}\n\nservice api from http {\n    json { options: plain }                 // ✓\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "json-for-non-bynk-clients",
    },
    Explain {
        code: "bynk.http.limit_bad_max_body",
        blurb: "`@limit` sets the byte ceiling above which the router answers `413`, so it must give a `maxBody`, and only the author can know the right one: it is a positive `Int` literal counting bytes. There is no size literal yet, so `\"1MB\"` or `1.mb` is refused; write the count, `1_048_576` for 1 MiB. The service-wide default is `limits { maxBody: … }`.",
        example: "@limit(maxBody: \"1MB\")    // ✗ a byte count, not a string\non POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@limit(maxBody: 1_048_576)   // ✓ 1 MiB\non POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }\n\nconst NoteCap: Int = 1_048_576\n@limit(maxBody: NoteCap)   // ✓ an `Int` constant bound to a literal\non POST(\"/drafts\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.http.limit_bad_max_file",
        blurb: "On a `@multipart` route, `@limit`'s `maxFile` caps the bytes of one file part and `maxParts` the number of parts. Each must be a positive `Int` literal: zero or a negative number would refuse every upload, and there is no size literal, so `\"5MB\"` is refused too. Write the count, as `5_000_000`. `limits { }` has the same rule (`bynk.http.limits_invalid_field`), and so does `maxBody` (`bynk.http.limit_bad_max_body`).",
        example: "consumes bynk { File }\n\ntype Upload = { photo: File }\n\n@multipart\n@limit(maxBody: 5_000_000, maxFile: 0)           // ✗ a positive byte count\non POST(\"/upload\") (body: Upload) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@multipart\n@limit(maxBody: 5_000_000, maxFile: 1_000_000)   // ✓\non POST(\"/upload\") (body: Upload) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.http.limit_duplicate",
        blurb: "A route has one request-body ceiling, so a handler carries one `@limit`. Two would set two ceilings for the same body with nothing to say which applies, and the second is refused. Keep the one you want. To set a default for every body-taking route of a service, write a `limits { }` section: a route's `@limit` overrides it. A handler may likewise carry only one `@cache` (`bynk.http.cache_duplicate`), `@raw` or `@json`.",
        example: "type Note = { text: String }\n\n@limit(maxBody: 1_000_000)\n@limit(maxBody: 5_000_000)    // ✗ a second `@limit`\non POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@limit(maxBody: 5_000_000)    // ✓ one ceiling\non POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.http.limit_on_bodyless",
        blurb: "`@limit` caps the size of a request body, so it means something only on a route that reads one: `POST`, `PUT` or `PATCH`. A `GET` or `DELETE` takes no body in Bynk, so there is nothing to cap. Move the annotation to the body-taking route, or remove it. A default for every body-taking route in a service goes in `limits { maxBody: … }`.",
        example: "@limit(maxBody: 1_000_000)   // ✗ a `GET` has no body\non GET(\"/notes\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\ntype Note = { text: String }\n\n@limit(maxBody: 1_000_000)   // ✓ a route that reads a body\non POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.http.limit_on_non_multipart",
        blurb: "`maxFile` and `maxParts` cap the parts of a multipart body: the size of one file and the number of parts. A route that is not `@multipart` has no parts, so naming either in `@limit` is refused. Remove the argument and keep `maxBody`, which caps any body, or mark the route `@multipart` if it takes an upload. A `@limit` on a route with no body at all is `bynk.http.limit_on_bodyless`.",
        example: "type Signup = { email: String }\n\n@form\n@limit(maxBody: 100_000, maxFile: 10_000)   // ✗ a form has no file parts\non POST(\"/signup\") (body: Signup) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@form\n@limit(maxBody: 100_000)                    // ✓\non POST(\"/signup\") (body: Signup) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.http.limit_unknown_arg",
        blurb: "`@limit` accepts `maxBody` and, on a `@multipart` route, `maxFile` and `maxParts`. The set is closed, so a misspelt or invented cap is refused rather than ignored, which would leave the route with a ceiling you did not set. Rename the argument or remove it. The ceilings are checked by `bynk.http.limit_bad_max_body` and `bynk.http.limit_bad_max_file`.",
        example: "type Note = { text: String }\n\n@limit(maxBody: 1_000_000, maxSize: 10)   // ✗ not an argument of `@limit`\non POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@limit(maxBody: 1_000_000)                // ✓\non POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.http.limits_invalid_field",
        blurb: "Every `limits { }` field is a positive `Int` literal: `maxBody` and `maxFile` are byte counts, and `maxParts` counts parts. A string such as `\"1MB\"`, zero or a negative number is refused: there is no size literal yet, and a ceiling of zero would reject every request. Write the count, as `1_048_576` for 1 MiB. A field outside the three is `bynk.http.limits_unknown_field`. The per-route form of the rule is `bynk.http.limit_bad_max_body`.",
        example: "type Note = { text: String }\n\nservice api from http {\n    limits { maxBody: \"1MB\" }       // ✗ a byte count, not a string\n    on POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }\n}\n\nservice api from http {\n    limits { maxBody: 1_048_576 }   // ✓ 1 MiB\n    on POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.http.limits_not_http",
        blurb: "A `limits { }` section sets the request-body ceilings of a service's routes, and only a `from http` service has routes that take requests, so on a `cron` or other service it is refused. Move it to the HTTP service it was meant for, or delete it. The `cors`, `security` and `json` sections have the same rule: `bynk.http.cors_not_http`, `bynk.http.security_not_http`, `bynk.http.json_not_http`.",
        example: "type Note = { text: String }\n\nservice jobs from cron {\n    limits { maxBody: 1_000_000 }   // ✗ a schedule takes no request body\n    on schedule(\"0 0 * * *\") () -> Effect[Result[(), String]] { … }\n}\n\nservice api from http {\n    limits { maxBody: 1_000_000 }   // ✓\n    on POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.http.limits_unknown_field",
        blurb: "A `limits { }` section sets the request-body ceilings of a service. Its fields are `maxBody` and, for `@multipart` routes, `maxFile` and `maxParts`. The set is closed, so a misspelt or invented field is refused rather than ignored, which would leave the routes with a ceiling you did not set. Rename it or remove it. A field with a bad value is `bynk.http.limits_invalid_field`; the per-route form is `bynk.http.limit_unknown_arg`.",
        example: "type Note = { text: String }\n\nservice api from http {\n    limits { maxBody: 1_000_000, maxSize: 10 }    // ✗ not a field of `limits`\n    on POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }\n}\n\nservice api from http {\n    limits { maxBody: 1_000_000 }                 // ✓\n    on POST(\"/notes\") (body: Note) -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.http.multipart_body_unbounded",
        blurb: "A `@multipart` body is buffered whole, with its parsed parts held beside it, so an upload with no ceiling lets one client use up the Worker's memory. A `@multipart` route therefore needs a cap, as a `@raw` one does (`bynk.http.raw_body_unbounded`): `@limit(maxBody: …)` on the handler, or `limits { maxBody: … }` on the service. Choose the largest upload you mean to accept.",
        example: "type Upload = { title: String }\n\n@multipart   // ✗ no cap on the handler or the service\non POST(\"/upload\") (body: Upload) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@multipart\n@limit(maxBody: 1_000_000)   // ✓ capped\non POST(\"/upload\") (body: Upload) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "multipart-bodies",
    },
    Explain {
        code: "bynk.http.path_param_not_stringy",
        blurb: "A path parameter is read from a segment of the URL, which is text, so its type must be one the router can build from a string: `String`, a refined `String`, or an opaque type over `String`. An `Int`, an enum or a record is refused here, though a `query` field may be one. Take the segment as a refined `String` that states its shape.",
        example: "on GET(\"/items/:id\") (id: Int) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✗ a path segment is a string\n\ntype ItemId = String where Matches(\"^[0-9]+$\")\non GET(\"/items/:id\") (id: ItemId) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.http.query_field_not_stringy",
        blurb: "A typed `query` record is filled from a query string, a set of keys with text values, so each field must be one a single value can fill: a `String`, `Int`, `Bool` or `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` or a `List` (a repeated key). A nested record or any other type is refused. Flatten the record, or take `query: RequestQuery` and read the keys yourself. A `@form` body has the same rule: `bynk.http.form_field_not_stringy`.",
        example: "type Range = { from: Int, to: Int }\ntype Filter = { range: Range }   // ✗ a query string has no nesting\n\ntype Filter = { from: Int, to: Int }   // ✓ flat keys",
        page: "/book/reference/http",
        anchor: "typed-query",
    },
    Explain {
        code: "bynk.http.query_not_http",
        blurb: "`RequestQuery` is the query string of an HTTP request. A handler that is not an `on http` route, such as a `cron` schedule, answers no request, so it has no query string to take. Remove the parameter, or, if the work is a reply to a caller, make it an `on http` handler. On an HTTP route, `query` is a record or `RequestQuery` and nothing else: `bynk.http.query_wrong_type`.",
        example: "service ticker from cron {\n    on schedule(\"*/5 * * * *\") (query: RequestQuery) -> Effect[Result[(), String]] { … }   // ✗ no request\n}\n\nservice ticker from http {\n    on GET(\"/ticks\") (query: RequestQuery) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓\n}",
        page: "/book/reference/http",
        anchor: "request-query",
    },
    Explain {
        code: "bynk.http.query_wrong_type",
        blurb: "A handler's `query` parameter is the request's query string, and the router offers two ways to take it: a record type, which it decodes and validates, answering `400` when the string does not fit, or `RequestQuery`, the raw view. Any other type is refused. Declare the keys you expect as a record, or take `RequestQuery`. The record's fields are held to `bynk.http.query_field_not_stringy`.",
        example: "on GET(\"/items\") (query: String) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✗ `query` is a record or `RequestQuery`\n\ntype Filter = { tag: Option[String] }\non GET(\"/items\") (query: Filter) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "typed-query",
    },
    Explain {
        code: "bynk.http.raw_bad_content_type",
        blurb: "`@raw(contentType: …)` is the media type the route accepts: the router compares it, ignoring parameters such as `charset`, with each request's `Content-Type` and answers `415` on a mismatch. So it is required, and must be a string literal of the form `type/subtype`. Write `\"image/png\"`, or `\"*/*\"` to accept any type.",
        example: "@raw(contentType: \"png\")         // ✗ not `type/subtype`\n@limit(maxBody: 1_000_000)\non POST(\"/avatars\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@raw(contentType: \"image/png\")   // ✓\n\nconst Png: String = \"image/png\"\n@raw(contentType: Png)   // ✓ a `String` constant bound to a literal\n@limit(maxBody: 1_000_000)\non POST(\"/icons\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "raw-request-bodies",
    },
    Explain {
        code: "bynk.http.raw_body_not_bytes",
        blurb: "`@raw` hands the handler the request's octets untouched, so its `body` must be `Bytes` (read whole) or `Stream[Bytes]` (read chunk by chunk); any other type would have to be decoded from them, and decoding is what `@raw` opts out of. Type `body` as `Bytes` and decode it in the handler, or drop `@raw` to have the router decode a JSON body for you. Without `@raw`, a `Bytes` body is base64 inside JSON.",
        example: "@raw(contentType: \"image/png\")\n@limit(maxBody: 1_000_000)\non POST(\"/avatars\") (body: String) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✗ raw octets are `Bytes`\n\n@raw(contentType: \"image/png\")\n@limit(maxBody: 1_000_000)\non POST(\"/avatars\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }    // ✓",
        page: "/book/reference/http",
        anchor: "raw-request-bodies",
    },
    Explain {
        code: "bynk.http.raw_body_unbounded",
        blurb: "A `@raw` route hands the handler its request body as untouched octets, \
                buffered in memory up to a cap. With no cap there is no bound on that \
                buffer, which is the denial of service request body limits exist to \
                stop, so a raw route must have one: a `@limit(maxBody:)` on the \
                handler, or a `limits { maxBody = … }` on the service.",
        example: "@raw(contentType: \"image/png\")           // ✗ no cap anywhere\n\
                  on POST(\"/avatars\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n\
                  @raw(contentType: \"image/png\")           // ✓ capped\n\
                  @limit(maxBody: 5_000_000)\n\
                  on POST(\"/avatars\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "raw-request-bodies",
    },
    Explain {
        code: "bynk.http.raw_duplicate",
        blurb: "`@raw` says the route reads its body as untouched bytes and names the one media type it accepts. A handler has one body, so a second `@raw` has no meaning: the router would have two media types to compare with and nothing to decide between them. Keep one. To accept more than one type, use `\"*/*\"`, or write a route for each. Another body annotation on the same route is `bynk.http.body_kind_conflict`.",
        example: "@raw(contentType: \"image/png\")\n@raw(contentType: \"image/jpeg\")    // ✗ a second `@raw`\n@limit(maxBody: 1_000_000)\non POST(\"/avatars\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@raw(contentType: \"*/*\")           // ✓ one `@raw`, any type\n@limit(maxBody: 1_000_000)\non POST(\"/avatars\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "raw-request-bodies",
    },
    Explain {
        code: "bynk.http.raw_error_status",
        blurb: "`RawError` answers with an error status and a body you render yourself, so its first argument must name a 4xx or 5xx `HttpResult` variant, such as `Forbidden` or `ErrorStatus.Conflict`, written as a name. A number, a 2xx name or an expression is refused, so a failure cannot be sent as a success. The diagnostic lists the names. A status name anywhere else is `bynk.http.error_status_position`.",
        example: "RawError(200, Bytes.empty(), \"text/plain\")        // ✗ the status is a name, not a number\n\nRawError(NotFound, Bytes.empty(), \"text/plain\")   // ✓",
        page: "/book/reference/http",
        anchor: "error-bodies",
    },
    Explain {
        code: "bynk.http.raw_on_bodyless",
        blurb: "`@raw` marks a request body as untouched octets, so it belongs on a `POST`, `PUT` or `PATCH` route with a `body: Bytes` parameter. On a `GET`, a `DELETE` or a route with no `body` there is nothing for it to mark. It is about the request, not the response: to answer with bytes, return `Raw(body, contentType)`. `@form` on such a route is `bynk.http.form_on_bodyless`.",
        example: "@raw(contentType: \"image/png\")   // ✗ a `GET` has no body\non GET(\"/avatars/:id\") (id: String) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@raw(contentType: \"image/png\")   // ✓ an upload route\n@limit(maxBody: 5_000_000)\non POST(\"/avatars\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "raw-request-bodies",
    },
    Explain {
        code: "bynk.http.raw_unknown_arg",
        blurb: "`@raw` takes one argument, `contentType:`, the media type the route accepts. The set is closed, so a misspelt or invented argument is refused rather than ignored, which would leave the route behaving otherwise than you wrote. A size ceiling is not an argument of `@raw`: it is a separate `@limit(maxBody: …)`. Remove the argument. A bad `contentType` is `bynk.http.raw_bad_content_type`.",
        example: "@raw(contentType: \"image/png\", maxSize: 10)   // ✗ not an argument of `@raw`\n@limit(maxBody: 1_000_000)\non POST(\"/avatars\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@raw(contentType: \"image/png\")                // ✓\n@limit(maxBody: 1_000_000)\non POST(\"/avatars\") (body: Bytes) -> Effect[HttpResult[String]] by v: Visitor { … }",
        page: "/book/reference/http",
        anchor: "raw-request-bodies",
    },
    Explain {
        code: "bynk.http.rejected_signature",
        blurb: "`on rejected` renders the router's own refusals, such as a `413` or a `405`, before any handler or actor exists, so it has a fixed shape: one parameter typed `BoundaryRejection`, returning `HttpResult[()]` or `Effect[HttpResult[()]]`, with no `by`, since no actor is known yet, and no `given`, since it only renders a body. Each departure is reported with the form to write. A second `on rejected` is `bynk.http.duplicate_rejected`.",
        example: "on rejected(why: String) -> Effect[HttpResult[()]] { … }             // ✗ the parameter is a `BoundaryRejection`\n\non rejected(why: BoundaryRejection) -> Effect[HttpResult[()]] { … }   // ✓",
        page: "/book/reference/http",
        anchor: "rendering-rejections",
    },
    Explain {
        code: "bynk.http.reserved_prefix",
        blurb: "Two path prefixes belong to the platform. `/_bynk/` is reserved for Bynk's own dispatch, and `/_assets/` is where content-hashed static assets are served before the Worker runs, so a handler there could be shadowed by any asset. A route at either, or at the bare `/_bynk` or `/_assets`, is refused. Choose another path.",
        example: "on GET(\"/_bynk/health\") () -> Effect[HttpResult[String]] by v: Visitor { … }   // ✗ reserved for the runtime\n\non GET(\"/health\") () -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.http.return_not_effect_http_result",
        blurb: "An `on http` handler returns `Effect[HttpResult[T]]`: `Effect` because it may use capabilities, and `HttpResult[T]` because the status is part of the answer (`Ok`, `Created`, `NotFound`, and so on). An `Effect[T]` with no status is refused. Wrap the type, and return one of the `HttpResult` variants.",
        example: "on GET(\"/items\") () -> Effect[String] by v: Visitor { … }   // ✗ no `HttpResult`\n\non GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.http.security_invalid_field",
        blurb: "The fields of a `security { }` section are fixed at compile time and become response headers. `hsts` must be a positive `Duration` literal such as `180.days`, because it is written as a `max-age` and a zero would clear the browser's HTTPS pin; a bare number has no unit. `nosniff` must be `true` or `false`. Write the value out. A field other than these two is `bynk.http.security_unknown_field`.",
        example: "service api from http {\n    security { hsts: 180 }         // ✗ a `Duration`, not a number\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}\n\nservice api from http {\n    security { hsts: 180.days }    // ✓\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "security-headers",
    },
    Explain {
        code: "bynk.http.security_not_http",
        blurb: "A `security { }` section sets the security headers, `Strict-Transport-Security` and `X-Content-Type-Options`, stamped on a service's HTTP responses. Only a `from http` service sends them, so on a `cron` or other service the section is refused. Move it to the HTTP service it was meant for, or delete it. The `cors`, `limits` and `json` sections have the same rule: `bynk.http.cors_not_http`, `bynk.http.limits_not_http`, `bynk.http.json_not_http`.",
        example: "service jobs from cron {\n    security { hsts: 180.days }   // ✗ a schedule sends no response headers\n    on schedule(\"0 0 * * *\") () -> Effect[Result[(), String]] { … }\n}\n\nservice api from http {\n    security { hsts: 180.days }   // ✓\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "security-headers",
    },
    Explain {
        code: "bynk.http.security_unknown_field",
        blurb: "A `security { }` section has two fields, `hsts` and `nosniff`, and each becomes a response header. The set is closed, so a misspelt or invented field is refused rather than ignored, which would leave the service sending headers you did not intend; no other header is set from this section. Rename the field or remove it. A field with a bad value is `bynk.http.security_invalid_field`.",
        example: "service api from http {\n    security { hsts: 180.days, frameOptions: \"DENY\" }    // ✗ `hsts` and `nosniff` only\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}\n\nservice api from http {\n    security { hsts: 180.days }                          // ✓\n    on GET(\"/items\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "security-headers",
    },
    Explain {
        code: "bynk.http.stream_body_not_raw",
        blurb: "A `Stream[Bytes]` `body` is a request body handed to the handler chunk by chunk, so the router must be told to leave it as octets: without `@raw` a body is JSON (or base64 inside JSON for `Bytes`), which has no streamed form. Add `@raw(contentType: \"…\")` and a `@limit`, or take the body as a value (`T`, or `Bytes`) to have it read whole.",
        example: "@limit(maxBody: 1_000_000)\non PUT(\"/blobs/:id\") (id: String, body: Stream[Bytes]) -> Effect[HttpResult[()]] by v: Visitor { … }   // ✗ not `@raw`\n\n@raw(contentType: \"application/octet-stream\")\n@limit(maxBody: 1_000_000)\non PUT(\"/blobs/:id\") (id: String, body: Stream[Bytes]) -> Effect[HttpResult[()]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "streamed-request-bodies",
    },
    Explain {
        code: "bynk.http.stream_body_on_sum_route",
        blurb: "A route `by` a sum of actors, such as `User | Visitor`, reads and verifies the request body itself before the handler runs. A streamed `body` is not read until the handler pulls it, so the two cannot both own it and the pair is refused. Name one actor on the route, or split it into one route per actor.",
        example: "actor User { auth = Bearer(secret = \"AUTH_JWT_SECRET\"), identity = UserId }\n\n@raw(contentType: \"image/png\")\n@limit(maxBody: 5_000_000)\non PUT(\"/avatars\") (body: Stream[Bytes]) -> Effect[HttpResult[()]] by who: User | Visitor { … }   // ✗ a sum of actors\n\n@raw(contentType: \"image/png\")\n@limit(maxBody: 5_000_000)\non PUT(\"/avatars\") (body: Stream[Bytes]) -> Effect[HttpResult[()]] by who: User { … }             // ✓ one actor",
        page: "/book/reference/http",
        anchor: "streamed-request-bodies",
    },
    Explain {
        code: "bynk.http.stream_body_signature",
        blurb: "A `Signature` actor checks an HMAC over the whole request body before the handler runs, so the body has to be held in full. A streamed `body` is read by the handler afterwards, a chunk at a time, and could not have been verified. Take `body: Bytes` on a signed route (a webhook, say), or sign the head (a header or the path) and verify the content another way.",
        example: "actor Hook { auth = Signature(secret = \"HOOK_SECRET\", header = \"X-Signature\") }\n\n@raw(contentType: \"application/octet-stream\")\n@limit(maxBody: 1_000_000)\non POST(\"/hook\") (body: Stream[Bytes]) -> Effect[HttpResult[()]] by h: Hook { … }   // ✗ unverifiable\n\n@raw(contentType: \"application/octet-stream\")\n@limit(maxBody: 1_000_000)\non POST(\"/hook\") (body: Bytes) -> Effect[HttpResult[()]] by h: Hook { … }          // ✓",
        page: "/book/reference/http",
        anchor: "streamed-request-bodies",
    },
    Explain {
        code: "bynk.http.unbound_path_param",
        blurb: "A `:name` segment in a route promises a value the handler can use, and the value reaches it only through a parameter of the same name. A segment with no such parameter would be matched and then thrown away, which is nearly always a slip, so the route is refused. Add the parameter, typed `String` or a refined `String`, or drop the segment from the path. The reverse, a parameter with no segment, is `bynk.http.extra_param`.",
        example: "on GET(\"/items/:id\") () -> Effect[HttpResult[String]] by v: Visitor { … }   // ✗ `:id` has no parameter\n\non GET(\"/items/:id\") (id: String) -> Effect[HttpResult[String]] by v: Visitor { … }   // ✓",
        page: "/book/reference/http",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.http.unknown_handler_annotation",
        blurb: "The handler annotations are a closed set: `@cache`, `@limit`, `@raw`, `@form`, `@multipart` and `@json`. Each changes how the router treats the route, so a misspelt one that was ignored would silently lose the behaviour you wanted, such as a freshness window or a body cap. Fix the spelling or remove the annotation. Settings shared by every route belong on the service, in a `cors`, `security`, `limits` or `json` block.",
        example: "@cached(maxAge: 5.minutes)   // ✗ no such annotation\non GET(\"/links\") () -> Effect[HttpResult[String]] by v: Visitor { … }\n\n@cache(maxAge: 5.minutes)    // ✓",
        page: "/book/reference/http",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.index.bad_argument",
        blurb: "`@indexed` says which field of the map's value type to maintain a secondary index on, and the only form it takes is `by: <field>`. A bare name, another label or an expression leaves the compiler without a field to index. Write `by:` followed by the field's name. A name that is not a field of the value type is `bynk.index.unknown_key`.",
        example: "type Order = { id: String, customerId: String }\n\nstore orders: Map[String, Order] @indexed(customerId)       // ✗ not a `by:` label\n\nstore orders: Map[String, Order] @indexed(by: customerId)   // ✓",
        page: "/book/reference/agents",
        anchor: "storage-annotations",
    },
    Explain {
        code: "bynk.index.unkeyable_key",
        blurb: "An index is looked up by equality on its key, so the field has to be a value the store can compare and key on: `Int`, `String`, or a refined or opaque type over one of them. A `Bool`, a record or a list cannot be. Index another field, or add a field of a keyable type to look up by. The same rule for a `Map` key is `bynk.store.unkeyable_key`.",
        example: "type Order = { id: String, paid: Bool, customerId: String }\n\nstore orders: Map[String, Order] @indexed(by: paid)         // ✗ a `Bool` is not keyable\n\nstore orders: Map[String, Order] @indexed(by: customerId)   // ✓ a `String`",
        page: "/book/reference/agents",
        anchor: "storage-annotations",
    },
    Explain {
        code: "bynk.invariant.cross_agent_reference",
        blurb: "An invariant is checked inside one agent's commit, against that agent's own state. Reading another agent would mean a call out to it, whose value could change before the commit lands, so the claim could not be held. A property that spans agents belongs in a saga or a scenario. Keep the invariant to this agent's fields. The mirror rule for steps is `bynk.transition.cross_agent_reference`.",
        example: "invariant within_total: count <= Total(\"all\").get()   // ✗ reads another agent\n\ninvariant non_negative: count >= 0                    // ✓ this agent's own state",
        page: "/book/reference/agent-invariants",
        anchor: "the-predicate-surface",
    },
    Explain {
        code: "bynk.invariant.duplicate_name",
        blurb: "Invariants are named so that a violation can say which one was broken. Two with the same name in one agent would make that report ambiguous. Rename one, or merge the two predicates if they are one claim. The same rule for steps is `bynk.transition.duplicate_name`.",
        example: "invariant bounded: count >= 0\ninvariant bounded: count < 100   // ✗ `bounded` is already declared\n\ninvariant capped: count < 100    // ✓ a distinct name",
        page: "/book/reference/agent-invariants",
        anchor: "the-predicate-surface",
    },
    Explain {
        code: "bynk.invariant.impure_predicate",
        blurb: "An invariant is checked at every commit, over the state about to be written, so it may only read: state fields, operators and pure value methods. `Effect.pure`, `?`, `expect`, `Val` and the other test-only forms are refused, as is anything that performs an effect. Rewrite the predicate as a plain `Bool` expression over the fields. The mirror rule for steps is `bynk.transition.impure_predicate`.",
        example: "invariant positive: Effect.pure(count >= 0)   // ✗ an effect, not a pure predicate\n\ninvariant positive: count >= 0                // ✓ a plain `Bool` expression",
        page: "/book/reference/agent-invariants",
        anchor: "the-predicate-surface",
    },
    Explain {
        code: "bynk.invariant.not_bool",
        blurb: "An invariant is a claim that is true or false of every committed state, so its predicate must be a `Bool`. A bare field such as `count` is a value, not a claim, and nothing says what is being asserted about it. Compare or test it: `count >= 0`. A `transition` is held to the same rule (`bynk.transition.not_bool`).",
        example: "invariant positive: count        // ✗ an `Int`, not a `Bool`\n\ninvariant positive: count >= 0   // ✓ a claim about `count`",
        page: "/book/reference/agent-invariants",
        anchor: "the-predicate-surface",
    },
    Explain {
        code: "bynk.json.encode_plain_generic",
        blurb: "`Json.encodePlain` writes an `Option` as its value or `null`, which it can do only when it knows the type. A generic function is compiled once for every `T`, and encodes through its `Codable` witness, which always writes `Option` in the tagged form `{\"kind\":\"Some\",…}`. Encode at a concrete type, or use `Json.encode` if the tagged form will do. A concrete type that is ambiguous in plain form is `bynk.json.nested_option_plain`.",
        example: "fn plain[T: Codable](v: T) -> String { Json.encodePlain(v) }   // ✗ `T` could be an `Option`\n\nfn plain(v: Option[Int]) -> String { Json.encodePlain(v) }     // ✓ a concrete type",
        page: "/book/reference/types",
        anchor: "plain-json",
    },
    Explain {
        code: "bynk.json.nested_option_plain",
        blurb: "Plain JSON writes `Some(v)` as `v` and `None` as `null`, so it can only be read back if `v` is never itself `null`. In an `Option[Option[T]]` or an `Option[()]`, `Some(None)` and `None` (or `Some(())` and `None`) would both be written `null`, and the decoder could not tell them apart. The compiler refuses the type wherever plain encoding is asked for: in the response type of a route whose `json { options: plain }` or `@json(options: plain)` applies, or in the argument of `Json.encodePlain`, through records, sums, containers and generic arguments alike. The tagged form, which `Json.encode` and a route's default use, writes `Option` as `{\"kind\":\"Some\",\"value\":…}` and has no such ambiguity. Either flatten the type, model the distinction as a sum type, or keep that route tagged.",
        example: "type Row = { id: String, note: Option[Option[String]] }\n\non GET(\"/rows\") () -> Effect[HttpResult[List[Row]]] by v: Visitor { … }   // ✗ `Some(None)` and `None` both write `null`\n\ntype Row = { id: String, note: Option[String] }   // ✓",
        page: "/book/reference/types",
        anchor: "plain-json",
    },
    Explain {
        code: "bynk.lambda.unannotated_param",
        blurb: "A lambda's parameter types come from where it is used: passed to `map` over a `List[Int]`, `(x) => x + 1` takes `x: Int`. A lambda standing alone, bound by a `let` for instance, has no such context, and the compiler asks for the type rather than guess it from the body. Annotate the parameter, or pass the lambda directly where a function type is expected.",
        example: "let f = (x) => x + x          // ✗ nothing says what `x` is\n\nlet f = (x: Int) => x + x     // ✓ annotated",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.lex.bad_escape",
        blurb: "A backslash in a string literal starts an escape, and this one is \
                not in the set: `\\n`, `\\r`, `\\t`, `\\0`, `\\\"`, `\\\\`, \
                `\\u{…}` (1 to 6 hex digits in braces) and `\\(…)` for \
                interpolation. To write a backslash itself, double it, as in a \
                Windows path. A `\\u{…}` naming a surrogate or a value past \
                U+10FFFF is `bynk.lex.bad_unicode_escape`.",
        example: "const Dir: String = \"C:\\Users\"      // ✗ `\\U` is not an escape\n\
                  const Dir: String = \"C:\\\\Users\"     // ✓ a doubled backslash",
        page: "/book/spec/lexical-grammar",
        anchor: "32-literals",
    },
    Explain {
        code: "bynk.lex.bad_unicode_escape",
        blurb: "A `\\u{…}` escape must name a Unicode scalar value, so a string \
                is always valid text. A surrogate (`D800` to `DFFF`) is half of \
                a UTF-16 pair and stands for no character on its own, and \
                anything above `10FFFF` is outside Unicode. Write the code point \
                of the character you mean, such as `\\u{E9}` for `é`. A `\\u` \
                with no braces, no digits or more than six digits is \
                `bynk.lex.bad_escape`.",
        example: "const Mark: String = \"\\u{D800}\"    // ✗ a surrogate is not a character\n\
                  const Mark: String = \"\\u{E9}\"      // ✓ `é`",
        page: "/book/spec/lexical-grammar",
        anchor: "322-string_literal",
    },
    Explain {
        code: "bynk.lex.float_literal_overflow",
        blurb: "A `Float` is a 64-bit IEEE 754 double, and a literal must be a \
                finite one: the largest is about 1.8e308. A literal past that \
                would round to infinity, which no `Float` literal can spell, so \
                it is refused rather than changed silently. Write a value in \
                range.",
        example: "const Big: Float = 1e999    // ✗ past the largest double\n\
                  const Big: Float = 1e308    // ✓",
        page: "/book/spec/lexical-grammar",
        anchor: "321a-float_literal",
    },
    Explain {
        code: "bynk.lex.integer_overflow",
        blurb: "An `Int` is a JavaScript safe integer, from -(2^53 - 1) to 2^53 - \
                1 (±9007199254740991), the range the runtime holds exactly; a \
                larger literal would round silently. The sign is a separate \
                operator, so the size of the digits decides. For a larger value \
                use a `Float` if rounding is acceptable, or a `String` for the \
                exact digits.",
        example: "const Big: Int = 9007199254740993    // ✗ past 2^53 - 1\n\
                  const Big: Int = 9007199254740991    // ✓ the largest `Int`",
        page: "/book/spec/lexical-grammar",
        anchor: "32-literals",
    },
    Explain {
        code: "bynk.lex.legacy_comment",
        blurb: "A `--` no longer starts a comment; a line comment is `//`. The old \
                spelling is refused rather than reread, so a file written for it \
                never changes meaning: `x = 1 -- note` is not quietly taken for `1 \
                - -note`. The fix is machine-applicable: `bynk check --fix` \
                applies it and `bynk fmt` rewrites a whole project. For a real `a \
                - -b`, write the space.",
        example: "const Limit: Int = 100 -- the cap     // ✗ `--` is the old comment\n\
                  const Limit: Int = 100        // ✓ a comment is `//`",
        page: "/book/spec/lexical-grammar",
        anchor: "333-retired-spellings",
    },
    Explain {
        code: "bynk.lex.legacy_doc_line",
        blurb: "A one-line doc is `--- text`. The older `--|` spelling is \
                refused rather than reread, so no file written for it changes \
                meaning unnoticed. The fix is machine-applicable: `bynk check \
                --fix` applies it and `bynk fmt` rewrites a whole project. A \
                `--|` with no text has no one-line form; write a doc block \
                between two `---` lines.",
        example: "--| Doubles a number.                  // ✗ the old spelling\n\
                  fn double(n: Int) -> Int { n * 2 }\n\
                  \n\
                  --- Doubles a number.                  // ✓\n\
                  fn double(n: Int) -> Int { n * 2 }",
        page: "/book/spec/lexical-grammar",
        anchor: "333-retired-spellings",
    },
    Explain {
        code: "bynk.lex.slash_doc",
        blurb: "A `///` is not a doc comment in Bynk. A doc is written with \
                dashes: `--- text` for one line, or a block between two `---` \
                lines for more. A plain comment is `//`. The fix to `---` is \
                machine-applicable: `bynk check --fix` applies it. A run of four \
                or more slashes is an ordinary comment.",
        example: "/// Doubles a number.                  // ✗ a comment, not a doc\n\
                  fn double(n: Int) -> Int { n * 2 }\n\
                  \n\
                  --- Doubles a number.                  // ✓\n\
                  fn double(n: Int) -> Int { n * 2 }",
        page: "/book/spec/lexical-grammar",
        anchor: "333-retired-spellings",
    },
    Explain {
        code: "bynk.lex.unclosed_doc_block",
        blurb: "A line of three or more dashes opens a documentation block, and \
                a line of exactly the same number of dashes closes it. Here no \
                closing line follows, so the compiler cannot tell where the \
                documentation ends. There is no standalone `---` divider; a \
                divider is a comment, `// ---`. Add the closing line.",
        example: "---                                // ✗ never closed\n\
                  Doubles a number.\n\
                  fn double(n: Int) -> Int { n * 2 }\n\
                  \n\
                  ---\n\
                  Doubles a number.\n\
                  ---                                // ✓ closed with as many dashes\n\
                  fn double(n: Int) -> Int { n * 2 }",
        page: "/book/spec/lexical-grammar",
        anchor: "332-doc-blocks",
    },
    Explain {
        code: "bynk.lex.unexpected_character",
        blurb: "The lexer met a character that starts no token in Bynk, such as \
                `#`, `$` or `'` outside a string literal. A comment is `//` (not \
                `#`) and a string takes double quotes (not single). The character \
                is skipped and lexing goes on, so every lexical error in the file \
                is reported together.",
        example: "const Limit: Int = 100 # the cap       // ✗ `#` is not a comment\n\
                  const Limit: Int = 100        // ✓ a comment is `//`",
        page: "/book/spec/lexical-grammar",
        anchor: "35-lexical-errors",
    },
    Explain {
        code: "bynk.lex.unterminated_string",
        blurb: "An ordinary string literal, `\"…\"`, closes on the line it opens on. \
                This one reaches the end of the line with no closing `\"`, so the \
                lexer reports it at the opening quote and skips the rest of the \
                line. Add the closing quote. For a line break in the value write \
                `\\n`, or use a multi-line string literal.",
        example: "const Greeting: String = \"hello      // ✗ no closing quote\n\
                  const Greeting: String = \"hello\"   // ✓",
        page: "/book/spec/lexical-grammar",
        anchor: "32-literals",
    },
    Explain {
        code: "bynk.messages.format_mismatch",
        blurb: "A placeholder that appears in the reference locale's template and in another \
                locale's must be formatted the same way in both: plain `{n}`, `plural`, `select`, \
                `number` or `date`. The caller supplies one kind of value for it, and an interface \
                cannot sensibly be plural-aware in one language and plain in another. Use the same \
                kind in every locale. The arms inside a `plural` or `select` may differ. A different \
                placeholder name is `bynk.messages.placeholder_mismatch`.",
        example: "messages \"en\" @reference {\n\
                \x20 \"cart.count\" => \"{n, plural, one {# item} other {# items}}\"\n\
                }\n\
                \n\
                messages \"fr\" {\n\
                \x20 \"cart.count\" => \"{n} articles\"        // ✗ plain here, plural in the reference\n\
                \x20 \"cart.count\" => \"{n, plural, one {# article} other {# articles}}\"  // ✓\n\
                }",
        page: "/book/guides/localisation/format-with-icu",
        anchor: "what-the-compiler-checks",
    },
    Explain {
        code: "bynk.messages.incomplete",
        blurb: "Every locale is held to the reference locale: it must supply each code the reference \
                declares, so that no locale silently lacks a message the reference has. The \
                diagnostic is reported once per missing code, at the locale's block. Add the missing \
                code to the locale. When you add a code to the reference, every other locale must \
                follow. A code that is present with different placeholders is \
                `bynk.messages.placeholder_mismatch`.",
        example: "messages \"en\" @reference {\n\
                \x20 \"cart.empty\" => \"Your basket is empty.\"\n\
                \x20 \"cart.greeting\" => \"Hello, {name}!\"\n\
                }\n\
                \n\
                messages \"fr\" {                         // ✗ lacks \"cart.greeting\"\n\
                \x20 \"cart.empty\" => \"Votre panier est vide.\"\n\
                \x20 \"cart.greeting\" => \"Bonjour, {name} !\"  // ✓ the code added\n\
                }",
        page: "/book/guides/localisation/declare-a-message-bundle",
        anchor: "adding-a-locale",
    },
    Explain {
        code: "bynk.messages.invalid_locale_tag",
        blurb: "A `messages` block's tag must be a `LocaleTag`: a well-formed BCP-47 tag in its \
                canonical casing, such as `en`, `fr`, `pt-BR` or `zh-Hans-CN`. The compiler checks \
                it because the runtime hands the tag to `Intl`, which throws on a malformed one, and \
                `render` must never fail. So `pt-br` is refused where `pt-BR` is accepted, and so is \
                a bare word such as `klingon`. Rewrite the tag in canonical form.",
        example: "messages \"pt-br\" @reference {           // ✗ a region is upper-case\n\
                \x20 \"cart.empty\" => \"A sua cesta está vazia.\"\n\
                }\n\
                \n\
                messages \"pt-BR\" @reference {           // ✓\n\
                \x20 \"cart.empty\" => \"A sua cesta está vazia.\"\n\
                }",
        page: "/book/guides/localisation/declare-a-message-bundle",
        anchor: "adding-a-locale",
    },
    Explain {
        code: "bynk.messages.malformed_icu_syntax",
        blurb: "A message template is checked as ICU MessageFormat at compile time, because a \
                malformed one would otherwise fail only when a request rendered it. A `plural` or \
                `select` must close its arm braces, use a known format keyword, give each arm a \
                distinct key and have an `other` arm, which is used when no other arm matches; `#` \
                stands for the number only inside a `plural` arm. Fix the template. Constructs the \
                compiler does not support, such as `selectordinal` and `offset:`, are reported here \
                too.",
        example: "messages \"en\" @reference {\n\
                \x20 \"cart.count\" => \"{n, plural, one {# item}}\"  // ✗ no `other` arm\n\
                \x20 \"cart.count\" => \"{n, plural, one {# item} other {# items}}\"  // ✓\n\
                }",
        page: "/book/guides/localisation/format-with-icu",
        anchor: "what-the-compiler-checks",
    },
    Explain {
        code: "bynk.messages.missing_locale_dependency",
        blurb: "A commons that declares `messages` gets a generated `render`, whose signature names \
                `LocaleTag` and `Message` and which falls back through `bynk.locale`. So that \
                commons must `uses` both locale units, as the example shows; add whichever is \
                missing. The context that uses the bundle should not `uses bynk.locale` itself: both \
                export a `render`, which is `bynk.uses.name_conflict`.",
        example: "commons shop.messages\n\
                \n\
                messages \"en\" @reference { … }          // ✗ no `uses` of bynk.locale\n\
                \n\
                uses bynk.locale                        // ✓ both lines are needed\n\
                uses bynk.locale.types\n\
                messages \"en\" @reference { … }",
        page: "/book/guides/localisation/declare-a-message-bundle",
        anchor: "the-smallest-bundle",
    },
    Explain {
        code: "bynk.messages.missing_reference",
        blurb: "A message bundle is a set of locale blocks, and the compiler checks every locale \
                against one of them, the reference, marked `@reference`. It is also the fallback \
                when a requested locale is not declared. With no reference there is nothing to check \
                the others against, so exactly one block per commons must carry the mark. Add \
                `@reference` to the locale your strings are written in first. Two marked blocks are \
                `bynk.messages.multiple_reference`.",
        example: "messages \"en\" {                         // ✗ no block is marked @reference\n\
                \x20 \"cart.empty\" => \"Your basket is empty.\"\n\
                }\n\
                \n\
                messages \"en\" @reference {              // ✓\n\
                \x20 \"cart.empty\" => \"Your basket is empty.\"\n\
                }",
        page: "/book/guides/localisation/declare-a-message-bundle",
        anchor: "the-smallest-bundle",
    },
    Explain {
        code: "bynk.messages.multiple_reference",
        blurb: "A message bundle is checked against one reference locale, which is also the fallback when a requested locale is not declared. With two blocks marked `@reference` there are two answers, so exactly one per commons may carry the mark. Keep `@reference` on the locale your strings are written in first and take it off the others. No block marked at all is `bynk.messages.missing_reference`.",
        example: "messages \"en\" @reference { … }\nmessages \"fr\" @reference { … }   // ✗ a second reference\n\nmessages \"fr\" { … }              // ✓",
        page: "/book/guides/localisation/declare-a-message-bundle",
        anchor: "the-smallest-bundle",
    },
    Explain {
        code: "bynk.messages.outside_commons",
        blurb: "A `messages` block is pure, stateless data, which is what a `commons` holds. A \
                context carries services and state and an adapter wraps foreign code, and neither \
                takes a bundle. Move the bundle into a commons, with the two `uses` lines it needs, \
                and have the context `uses` that commons.",
        example: "context shop.web\n\
                \n\
                messages \"en\" @reference { … }          // ✗ a context cannot hold a bundle\n\
                \n\
                commons shop.messages\n\
                \n\
                uses bynk.locale\n\
                uses bynk.locale.types\n\
                messages \"en\" @reference { … }          // ✓ a commons can",
        page: "/book/guides/localisation/declare-a-message-bundle",
        anchor: "the-smallest-bundle",
    },
    Explain {
        code: "bynk.messages.placeholder_mismatch",
        blurb: "A caller supplies a message's arguments by name, so every locale's template for a \
                code must use the same set of `{name}` placeholders as the reference locale's. A \
                translation that renames, drops or adds one would ask for an argument the caller \
                never passes. Order is free: translations may reorder placeholders to suit their \
                grammar. Rename the placeholder to match the reference. A placeholder of the right \
                name but a different ICU kind is `bynk.messages.format_mismatch`.",
        example: "messages \"en\" @reference {\n\
                \x20 \"cart.greeting\" => \"Hello, {name}!\"\n\
                }\n\
                \n\
                messages \"fr\" {\n\
                \x20 \"cart.greeting\" => \"Bonjour, {prenom} !\"  // ✗ the reference uses {name}\n\
                \x20 \"cart.greeting\" => \"Bonjour, {name} !\"    // ✓\n\
                }",
        page: "/book/guides/localisation/declare-a-message-bundle",
        anchor: "adding-a-locale",
    },
    Explain {
        code: "bynk.mysql.sql_invalid",
        blurb: "When the project has a `schema.sql` at its root, Bynk parses each literal SQL statement of a `MySql` capability, so a syntax error shows up when you check, not on the first request. It also flags a statement of several commands, which the driver does not accept: send each with its own call, or use `transaction`. The parser is a pure-Rust one with a MySQL dialect, not MySQL's own (and MariaDB has forms of its own), so it can refuse valid SQL; that is why this is a warning and never blocks a build. Nothing runs, and the statement is not checked against the schema, which Bynk does not own. Correct the statement, or delete `schema.sql` to turn the check off.",
        example: "let _ <- Legacy.all(\"SELEC id FROM orders\", [])        // ✗ expected a statement, found SELEC\n\nlet _ <- Legacy.all(\"SELECT id FROM orders\", [])      // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "mysql-sql-checked-for-syntax",
    },
    Explain {
        code: "bynk.mysql.sql_param_count",
        blurb: "When the project has a `schema.sql`, Bynk counts the parameters of each literal `MySql` statement: each `?` takes the next value. It checks a list written out in place, or a name `let`-bound to one. A `?` inside a single-quoted string literal, a backtick-quoted name or a comment (`-- ` or `/* … */`) is text, not a parameter, as the driver reads it; the driver does not skip a double-quoted string or a `#` comment, so a `?` in one is counted, and string literals are written with single quotes. Pass one `MySqlValue` for each `?`, or change the SQL. `??`, which mysql2 reads as an identifier placeholder, is counted as two parameters and is not supported.",
        example: "let _ <- Legacy.all(\"SELECT id FROM orders WHERE id = ? AND qty > ?\", [MySqlInteger(id)])                    // ✗ two parameters, one value\n\nlet _ <- Legacy.all(\"SELECT id FROM orders WHERE id = ? AND qty > ?\", [MySqlInteger(id), MySqlInteger(5)])    // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "mysql-sql-checked-for-syntax",
    },
    Explain {
        code: "bynk.namespace.reserved",
        blurb: "The `bynk` root belongs to the toolchain: its own libraries, such as `bynk.list`, and the platform adapters live there and are added to a build when a unit consumes them. A unit of yours named `bynk` or `bynk.<anything>` could collide with one of them, so the name is refused for every kind of unit. Rename the unit so its first segment is not `bynk`, and move its file to match.",
        example: "// bynk/foo.bynk\ncommons bynk.foo    // ✗ `bynk` is the toolchain's namespace\n\n// shop/foo.bynk\ncommons shop.foo    // ✓",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.observe.impure_with",
        blurb: "The `with` predicate of `expect Cap.op called with …` is tested against every recorded call, so it must be a pure expression of the operation's arguments. `trace(…)`, a capability call or anything else effectful or test-only could give a different answer each time. Compare the arguments, or bind `trace(Cap.op)` first and assert on the list.",
        example: "// tests/t.test.bynk\nexpect Logger.log called with trace(Logger.log).length() > 0   // ✗ not a pure predicate\nexpect Logger.log called with msg == \"x\"                      // ✓",
        page: "/book/reference/testing",
        anchor: "observation",
    },
    Explain {
        code: "bynk.observe.not_a_seam",
        blurb: "`expect Cap.op called` checks the calls the unit under test made through a capability it consumes, whether declared in its context, brought in with `consumes`, or listed in `given`. A name that is not one of those, such as a type or a function, has no recorded calls. Name a consumed capability. An operation it does not declare is `bynk.observe.unknown_op`.",
        example: "// tests/t.test.bynk\nexpect AuthId.unsafe called   // ✗ `AuthId` is a type, not a capability\nexpect Logger.log called      // ✓",
        page: "/book/reference/testing",
        anchor: "observation",
    },
    Explain {
        code: "bynk.observe.trace_outside_test",
        blurb: "`trace(Cap.op)` reads the calls a test has recorded through a capability. Only a test records them, so it is allowed only in a `case` body, never in a service, an agent or a function. Remove it from the code under test and read the trace in the test, or observe the call with `expect Cap.op called`.",
        example: "// demo/pay.bynk\nlet calls = trace(Logger.log)               // ✗ in the service\n\n// tests/t.test.bynk\nexpect trace(Logger.log).length() == 1      // ✓ in a case",
        page: "/book/reference/testing",
        anchor: "observation",
    },
    Explain {
        code: "bynk.observe.unknown_op",
        blurb: "`expect Cap.op called` checks the calls recorded for one \
                operation of a capability, so `op` must be declared by that \
                capability. A misspelt or renamed operation would match no \
                call, so it is refused instead of leaving the check vacuous. \
                Use the name the capability declares. A capability the unit \
                does not consume is `bynk.observe.not_a_seam`.",
        example: "// demo/pay.bynk\n\
                  capability Logger { … }\n\n\
                  // tests/pay.test.bynk\n\
                  expect Logger.nope called    // ✗ Logger has no `nope`\n\
                  expect Logger.log called     // ✓",
        page: "/book/reference/testing",
        anchor: "observation",
    },
    Explain {
        code: "bynk.observe.with_not_bool",
        blurb: "The `with` clause of `expect Cap.op called with …` filters the recorded calls, keeping those for which it is true, so it must be a `Bool`. A bare argument name is a value, not a condition. Compare it with something: `with msg == \"x\"`.",
        example: "// tests/t.test.bynk\nexpect Logger.log called with msg           // ✗ a `String`, not a `Bool`\nexpect Logger.log called with msg == \"x\"   // ✓",
        page: "/book/reference/testing",
        anchor: "observation",
    },
    Explain {
        code: "bynk.package.alias_shadows_unit",
        blurb: "The key of a `[dependencies]` entry is the alias a unit reaches the package by, as in `lib.notify`. If one of the project's own units also starts with that word, `shop.cart` for an alias `shop`, then `shop.…` could mean the package or the unit, and the compiler will not choose. Rename the alias, which is the key, or rename the unit.",
        example: "[dependencies]\nshop = { package = \"acme/lib\", path = \"pkg/lib\" }   // ✗ the project has a unit `shop`\n\nlib = { package = \"acme/lib\", path = \"pkg/lib\" }    // ✓ an alias no unit starts with",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "what-the-checks-catch",
    },
    Explain {
        code: "bynk.package.dependency_not_found",
        blurb: "A path dependency points at a directory that is a package: it has its own `bynk.toml` with a `[package]` table. The path is read from the manifest that names it. The entry is refused when there is no readable `bynk.toml` there, when it is not valid, or when it has no `[package]`. Correct the path, or add the manifest to the package.",
        example: "[dependencies]\nlib = { package = \"acme/lib\", path = \"pkg/libs\" }   // ✗ no `bynk.toml` at `pkg/libs`\n\nlib = { package = \"acme/lib\", path = \"pkg/lib\" }    // ✓ the package's directory",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "depend-on-it",
    },
    Explain {
        code: "bynk.package.dev_dependency_in_source",
        blurb: "A `[dev-dependencies]` package is there for the project's suites, and a deployable build leaves suites out, so a package that only suites need is never compiled into it. A context or commons that reached one would break that build, so only a suite may. If the source needs the package, move its entry to `[dependencies]`; otherwise keep the use in a suite.",
        example: "// bynk.toml\n[dev-dependencies]\nlib = { package = \"acme/lib\", path = \"pkg/lib\" }\n\n// shop.bynk\nuses lib.notify      // ✗ a `[dev-dependencies]` alias is for suites\n\n// bynk.toml\n[dependencies]       // ✓ the entry moved here\nlib = { package = \"acme/lib\", path = \"pkg/lib\" }",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "dependencies-only-tests-use",
    },
    Explain {
        code: "bynk.package.duplicate_source",
        blurb: "A build holds one copy of each package, so every dependency on a package must reach it in the same directory. Two entries that name one package but point at different directories would give two copies, with two sets of types where there should be one. Point both at the same directory, or drop one. Two aliases onto one directory are fine.",
        example: "[dependencies]\nlib = { package = \"acme/lib\", path = \"pkg/lib\" }\nutil = { package = \"acme/lib\", path = \"vendor/lib\" }   // ✗ `acme/lib` from a second directory\n\nutil = { package = \"acme/lib\", path = \"pkg/lib\" }      // ✓ the same directory",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "what-the-checks-catch",
    },
    Explain {
        code: "bynk.package.name_mismatch",
        blurb: "A path dependency gives the package's name twice: `package = \"acme/lib\"` in your `[dependencies]`, and `name` under `[package]` in the manifest at that path. They must agree, so that what you asked for is what you get. A mismatch usually means the path is the wrong directory, or the package was renamed. Correct whichever is wrong.",
        example: "[dependencies]\nmoney = { package = \"acme/money\", path = \"pkg/lib\" }   // ✗ `pkg/lib` is the package `acme/lib`\n\nmoney = { package = \"acme/lib\", path = \"pkg/lib\" }     // ✓ the name it declares",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "what-the-checks-catch",
    },
    Explain {
        code: "bynk.package.unit_not_exported",
        blurb: "A package lists in `exports` the units it offers to its users, and keeps the rest free to change, as a library keeps its internals. A unit that exists but is not listed is refused, so a consumer cannot come to rely on it. Use one of the exported units, or ask the package to export it. Leaving `exports` out shares every commons and adapter. A name the package does not declare at all is `bynk.package.unknown_unit`.",
        example: "exports = [\"notify\"]      // in the package's `bynk.toml`\n\nuses lib.internal     // ✗ not in `exports`\n\nuses lib.notify       // ✓ an exported unit",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "what-the-checks-catch",
    },
    Explain {
        code: "bynk.package.unknown_unit",
        blurb: "A dependency is reached as `alias.unit`, where the unit is a commons or adapter the package declares. A name after the alias that matches none of them is refused, most often because it is misspelt. The diagnostic's note lists the units the package shares. A unit that exists but is not exported is `bynk.package.unit_not_exported`.",
        example: "commons notify          // in the package `acme/lib`\n\nuses lib.notifiy      // ✗ the package declares no `notifiy`\n\nuses lib.notify       // ✓",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "what-the-checks-catch",
    },
    Explain {
        code: "bynk.package.unsupported_in_dependency",
        blurb: "A dependency may not declare an `asset` or an `embed`, because the path would be resolved against the project that consumes it, whose `assets/` or `embeds/` folder is not the package's. Move the file into your own project, and have the package take its content as an argument.",
        example: "commons board                           // in the package `acme/lib`\n\nasset style = \"assets/board.css\"        // ✗ a dependency cannot declare one\nfn sheet() -> String { style.url }\n\ncommons board\n\nfn sheet(url: String) -> String { url } // ✓ the project supplies the URL",
        page: "/book/reference/assets",
        anchor: "declaring-an-asset",
    },
    Explain {
        code: "bynk.par.invalid_statement",
        blurb: "A `par { … }` block issues all its bindings together and waits for them, so it holds only `let x <- e` and `do e`. Anything else has no effect to overlap, nothing to join, or must stay in order: a plain `let`, `~>`, `:=` or a `store` write, `expect`, `advance`, a nested `par`, a `let … else`, a bare expression (write `do e`) and an empty block. Store writes stay sequential; for `parTraverse` closures the rule is `bynk.par.store_write_in_closure`. Move the statement before or after the block; the names a block binds are in scope after its closing brace. See also `bynk.par.sibling_name` (a binding reading another binding of the same block) and the warning `bynk.par.single_binding`.",
        example: "par {\n  let a <- Users.get(id)\n  let b = 1   // ✗ a plain `let` has no effect to overlap\n}\n\nlet b = 1   // ✓ compute it before the block\npar {\n  let a <- Users.get(id)\n  let c <- Users.get(b)\n}",
        page: "/book/reference/capabilities",
        anchor: "par",
    },
    Explain {
        code: "bynk.par.question_in_join",
        blurb: "The bindings of a `par` block are issued together and the block waits for all of them, so a `?` inside one has no single place to return from. Bind each effect's `Result` in the block and apply `?` after it, where the lines run in source order and the first `Err` propagates. A `?` on an argument can be computed before the block instead.",
        example: "par {\n    let a <- Users.find(1)\n    let b <- Orders.forUser(parse(raw)?)   // ✗ `?` inside the block\n}\n\nlet key = parse(raw)?                      // ✓ unwrap it before the block\npar {\n    let a <- Users.find(1)\n    let b <- Orders.forUser(key)\n}",
        page: "/book/reference/capabilities",
        anchor: "par",
    },
    Explain {
        code: "bynk.par.sibling_name",
        blurb: "The bindings in a `par { … }` block run together, so none can read the result of another: there is no order in which one could be given the other's value. A binding that mentions a name a sibling binds is refused, even where an outer binding of that name exists, so a stale value is never picked up silently. Binding one name twice in a block is refused as well, since later code could not tell which result is meant. Move the dependent binding after the block, where every name is in scope, or rename.",
        example: "par {\n    let a <- Effect.pure(1)\n    let b <- Effect.pure(a + 1)   // ✗ `a` is bound by a sibling\n}\n\npar {\n    let a <- Effect.pure(1)\n    let c <- Effect.pure(2)\n}\nlet b <- Effect.pure(a + 1)   // ✓ after the block, `a` is in scope",
        page: "/book/reference/capabilities",
        anchor: "par",
    },
    Explain {
        code: "bynk.par.store_write_in_closure",
        blurb: "The closure given to `parTraverse`, `parTraverseAll` or `parTraverseTry` runs \
                once per element, all at once, so it may not write a `store` field, which is \
                the rule a `par { … }` block already follows. Every `<-` is an await point, \
                so a `get` then a `put` in the closure lets the other branches write in \
                between, and their updates are lost. A single call such as `update` is atomic \
                per key, but it is refused here too, so the two rules stay equal. Run the \
                loop with `forEach` (or `traverseAll` / `traverseTry`), where each element \
                finishes before the next starts, so a single-call `update` stays atomic. A store \
                write reads the entry before it stages the write, so the sequential form \
                reads one entry at a time; per-key atomicity is the reason for the rule. To \
                keep the concurrent part, collect with `parTraverseAll` and write afterwards.",
        example: "xs.parTraverse((x: Int) => {\n  let o <- counts.get(\"k\")\n  counts.put(\"k\", o.getOrElse(0) + 1)   // ✗ two increments of three are lost\n})\n\n\
                  xs.forEach((x: Int) => counts.update(\"k\", (v) => v + 1))   // ✓ sequential and atomic per key",
        page: "/book/reference/capabilities",
        anchor: "par",
    },
    Explain {
        code: "bynk.parse.bare_expression_statement",
        blurb: "A block has no expression statements: each statement is a `let`, `do`, \
                `~>`, `expect`, `:=` and so on, and the last expression is the block's \
                value. An expression written on its own line with more of the block after \
                it would have its value thrown away, so the compiler asks you to say what \
                happens to it. Bind it: `let _ = expr` when you only want to discard a \
                value, or `let _ <- expr` when it is an effect to run and await (a call \
                through a capability or an agent, or a cell's `.update`). The parser \
                cannot see types, so it picks the `<-` form for a call that looks like an \
                effect and the `=` form otherwise, and its note names the other. The fix \
                is machine-applicable, so `bynk check --fix` applies it.",
        example: "on call bump() -> Effect[Result[Int, String]] {\n    \
                    count.update((c) => c + 1)            // ✗ an expression standing alone\n    \
                    Ok(count)\n}\n\n\
                  on call bump() -> Effect[Result[Int, String]] {\n    \
                    let _ <- count.update((c) => c + 1)   // ✓ the effect is bound\n    \
                    Ok(count)\n}\n\n\
                  let _ = a + b                           // ✓ a plain value you mean to discard",
        page: "/book/spec/syntactic-grammar",
        anchor: "",
    },
    Explain {
        code: "bynk.parse.clause_after_statement",
        blurb: "A `stub` or `env` clause sets up the world a test case runs in, \
                so it comes at the start of the case body, before any statement. \
                One after a `let` or other statement is refused, so the setup \
                reads in one place above the steps that depend on it. Move the \
                clause above the first statement of the case.",
        example: "suite demo {\n\
                  \x20\x20case \"x\" {\n\
                  \x20\x20\x20\x20let k = 1\n\
                  \x20\x20\x20\x20stub Clock.now() returns 0       // ✗ after a statement\n\
                  \x20\x20\x20\x20let t <- probe.call()\n\
                  \x20\x20\x20\x20expect t == 0\n\
                  \x20\x20}\n\
                  }\n\
                  \n\
                  suite demo {\n\
                  \x20\x20case \"x\" {\n\
                  \x20\x20\x20\x20stub Clock.now() returns 0       // ✓ before the statements\n\
                  \x20\x20\x20\x20let k = 1\n\
                  \x20\x20\x20\x20let t <- probe.call()\n\
                  \x20\x20\x20\x20expect t == 0\n\
                  \x20\x20}\n\
                  }",
        page: "/book/reference/testing",
        anchor: "",
    },
    Explain {
        code: "bynk.parse.const_missing_type",
        blurb: "A `const` always states its type: `const Limit: Int = 100`. A \
                constant is part of its unit's interface, so a reader, or a unit \
                that `uses` it, can see its type without inferring it from the \
                initialiser. Write the type after the name, before the `=`.",
        example: "const Limit = 100          // ✗ no type annotation\n\
                  const Limit: Int = 100     // ✓",
        page: "/book/reference/constants",
        anchor: "declaring-a-constant",
    },
    Explain {
        code: "bynk.parse.consumes_after_decls",
        blurb: "In the fragment form of a `context` (the header with no braces), \
                the `consumes` clauses come straight after the `uses` lines, \
                before any `type` or `fn`, so a unit's dependencies are all \
                stated at the top. Move the `consumes` line up. `uses` and \
                `exports` follow the same rule, as `bynk.parse.uses_after_decls` \
                and `bynk.parse.exports_after_decls`.",
        example: "context shop.orders\n\
                  type Id = String\n\
                  consumes shop.payment            // ✗ a declaration comes first\n\
                  \n\
                  context shop.orders\n\
                  consumes shop.payment            // ✓ clauses straight after the header\n\
                  type Id = String",
        page: "/book/spec/syntactic-grammar",
        anchor: "",
    },
    Explain {
        code: "bynk.parse.dangling_handler_annotation",
        blurb: "A handler annotation, such as `@cache(maxAge: 5.minutes)`, \
                applies to the handler after it, so it must sit directly above \
                an `on`. Here something else, or the end of the service, follows \
                it. Move the annotation to just above its handler.",
        example: "service api from http {\n\
                  \x20\x20\x20\x20on GET(\"/rows\") () -> Effect[HttpResult[List[Row]]] by v: Visitor { … }\n\
                  \x20\x20\x20\x20@cache(maxAge: 5.minutes)\n\
                  }                                  // ✗ the annotation has no handler to apply to\n\
                  \n\
                  service api from http {\n\
                  \x20\x20\x20\x20@cache(maxAge: 5.minutes)      // ✓ directly above its handler\n\
                  \x20\x20\x20\x20on GET(\"/rows\") () -> Effect[HttpResult[List[Row]]] by v: Visitor { … }\n\
                  }",
        page: "/book/spec/static-semantics",
        anchor: "cache",
    },
    Explain {
        code: "bynk.parse.doc_block_contains_code",
        blurb: "A line of three or more dashes opens or closes a documentation \
                block, so two dash lines turn the code between them into \
                documentation, and it is never compiled. Use a line comment for a \
                divider. To show code in a documentation block, put it in a \
                Markdown code fence.",
        example: "--------                             // ✗ opens a doc block, not a divider\n\
                  fn double(n: Int) -> Int { n * 2 }\n\
                  --------\n\
                  fn triple(n: Int) -> Int { n * 3 }\n\n\
                  ---\n\
                  Doubles a number.                    // ✓ a doc block holds prose\n\
                  ---\n\
                  fn double(n: Int) -> Int { n * 2 }",
        page: "/book/spec/lexical-grammar",
        anchor: "332-doc-blocks",
    },
    Explain {
        code: "bynk.parse.duplicate_cors",
        blurb: "A service has one CORS policy, and it applies to every route in it. Two `cors` blocks could grant different things, and the reader would not know which holds, so the second is refused. Merge them into one. `security` and `limits` follow the same rule (`bynk.parse.duplicate_security`, `bynk.parse.duplicate_limits`), as does `json` (`bynk.parse.duplicate_json`).",
        example: "service api from http {\n    cors { origins: [\"https://a.example\"] }\n    cors { origins: [\"https://b.example\"] }          // ✗ a second `cors`\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}\n\nservice api from http {\n    cors { origins: [\"https://a.example\", \"https://b.example\"] }   // ✓ one policy\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "cors",
    },
    Explain {
        code: "bynk.parse.duplicate_json",
        blurb: "A service's `json` policy sets whether its response bodies use plain or tagged JSON, so there is one setting for the whole service. A second block would contradict the first, and is refused. Keep one. A single route can still differ with `@json` on its handler. The same rule holds for `cors`, `security` and `limits`, as `bynk.parse.duplicate_cors` shows.",
        example: "service api from http {\n    json { options: plain }\n    json { options: tagged }                         // ✗ a second `json`\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}\n\nservice api from http {\n    json { options: plain }                          // ✓ one setting\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "json-for-non-bynk-clients",
    },
    Explain {
        code: "bynk.parse.duplicate_limits",
        blurb: "A service has one `limits` policy, and it sets the body-size cap for its body-taking routes. Two blocks could name different caps, and the reader would not know which holds, so the second is refused. Merge them into one, keeping the cap you mean. A single route can still differ with `@limit` on its handler. `cors`, `security` and `json` follow the same rule (`bynk.parse.duplicate_cors`, `bynk.parse.duplicate_security`, `bynk.parse.duplicate_json`).",
        example: "service api from http {\n    limits { maxBody: 1_048_576 }\n    limits { maxBody: 4_194_304 }                    // ✗ a second `limits`\n    on POST(\"/a\") (body: Int) -> Effect[HttpResult[Int]] by Visitor { … }\n}\n\nservice api from http {\n    limits { maxBody: 4_194_304 }                    // ✓ one cap\n    on POST(\"/a\") (body: Int) -> Effect[HttpResult[Int]] by Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "request-body-limits",
    },
    Explain {
        code: "bynk.parse.duplicate_security",
        blurb: "A service has one `security` policy, and it sets the security headers (`hsts`, `nosniff`) stamped on every response. Two blocks could set them differently, and the reader would not know which holds, so the second is refused. Merge them into one. `cors`, `limits` and `json` follow the same rule (`bynk.parse.duplicate_cors`, `bynk.parse.duplicate_limits`, `bynk.parse.duplicate_json`).",
        example: "service api from http {\n    security { hsts: 180.days }\n    security { nosniff: true }                       // ✗ a second `security`\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}\n\nservice api from http {\n    security { hsts: 180.days, nosniff: true }       // ✓ one policy\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "security-headers",
    },
    Explain {
        code: "bynk.parse.empty_agent",
        blurb: "An agent is its storage and the `on call` handlers that read and \
                change it. One with no handler can never be called, so it is \
                refused, as is one with no `store` field \
                (`bynk.parse.expected_agent_storage`). Add a handler.",
        example: "agent Tally {                                              // ✗ no handler\n\
                  \x20\x20\x20\x20key id: String\n\
                  \x20\x20\x20\x20store count: Cell[Int]\n\
                  }\n\
                  \n\
                  agent Tally {\n\
                  \x20\x20\x20\x20key id: String\n\
                  \x20\x20\x20\x20store count: Cell[Int]\n\
                  \n\
                  \x20\x20\x20\x20on call get() -> Effect[Result[Int, String]] {         // ✓\n\
                  \x20\x20\x20\x20\x20\x20\x20\x20Ok(count)\n\
                  \x20\x20\x20\x20}\n\
                  }",
        page: "/book/reference/agents",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.parse.empty_capability",
        blurb: "A capability is a set of operations that a unit asks for with \
                `given` and that a provider supplies. With no operations there \
                is nothing to call or to provide, so an empty one is refused. \
                Declare at least one operation, or remove the capability.",
        example: "capability Clock {                 // ✗ no operations\n\
                  }\n\
                  \n\
                  capability Clock {\n\
                  \x20\x20\x20\x20fn now() -> Effect[Int]        // ✓\n\
                  }",
        page: "/book/reference/capabilities",
        anchor: "declaring-a-capability",
    },
    Explain {
        code: "bynk.parse.empty_interpolation",
        blurb: "An interpolation hole, `\\(…)`, holds one expression, whose \
                value is written into the text. An empty `\\()` has nothing to \
                write. Put an expression in the hole. To write the characters \
                `\\(` themselves, escape the backslash: `\\\\(`.",
        example: "fn label(n: Int) -> String {\n\
                  \x20\x20\x20\x20\"total: \\()\"                   // ✗ nothing to interpolate\n\
                  }\n\
                  \n\
                  fn label(n: Int) -> String {\n\
                  \x20\x20\x20\x20\"total: \\(n)\"                  // ✓\n\
                  }",
        page: "/book/spec/lexical-grammar",
        anchor: "322a-string_interpolation",
    },
    Explain {
        code: "bynk.parse.empty_match",
        blurb: "A `match`, or the `else match` of a `let … else`, must have at \
                least one arm. Each arm gives the match a value, so one with no \
                arms has none to give. Write an arm for each shape the value can \
                take; a final `_ =>` arm takes whatever the others leave.",
        example: "match xs {           // ✗ no arms\n\
                  }\n\n\
                  match xs {\n    \
                  [x, ..] => x      // ✓ one arm for each shape\n    \
                  [] => 0\n\
                  }",
        page: "/book/reference/types",
        anchor: "matching",
    },
    Explain {
        code: "bynk.parse.empty_service",
        blurb: "A service is its handlers, so one with none can never be called \
                or triggered, and is refused. (A service that subscribes to \
                `Events` takes exactly one `on event`.) Add a handler, or remove \
                the service.",
        example: "service orders {                   // ✗ no handler\n\
                  }\n\
                  \n\
                  service orders {\n\
                  \x20\x20\x20\x20on call() -> Effect[Int] {     // ✓\n\
                  \x20\x20\x20\x20\x20\x20\x20\x20Effect.pure(0)\n\
                  \x20\x20\x20\x20}\n\
                  }",
        page: "/book/spec/syntactic-grammar",
        anchor: "441-service_decl",
    },
    Explain {
        code: "bynk.parse.event_pattern_empty",
        blurb: "A pattern in `from Events(E { … })` narrows delivery to events whose fields match, so it must name at least one field to match on. An empty `E {}` filters nothing and only looks as if it does. For every event of the type, drop the braces: `from Events(E)`.",
        example: "service OnPaid from Events(Paid {}) {         // ✗ a pattern with no fields\n    on event(e: Paid) -> Effect[()] { … }\n}\n\nservice OnPaid from Events(Paid) {            // ✓ every `Paid`\n    on event(e: Paid) -> Effect[()] { … }\n}",
        page: "/book/guides/events/understand-events",
        anchor: "filtering-delivery-with-a-pattern",
    },
    Explain {
        code: "bynk.parse.expected_agent_key",
        blurb: "An agent is addressed by its key, so the key is the first thing in its body: `key id: Type`. Anything else there, usually a `store` field written first, is refused, so that how an agent is found is the first thing a reader sees. Move the `key` line to the top.",
        example: "agent Counter {\n    store n: Cell[Int]                   // ✗ the key comes first\n    key id: String\n\n    on call get() -> Effect[Int] { … }\n}\n\nagent Counter {\n    key id: String                       // ✓\n    store n: Cell[Int]\n\n    on call get() -> Effect[Int] { … }\n}",
        page: "/book/reference/agents",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.parse.expected_agent_storage",
        blurb: "An agent exists to hold state, and its state is its `store` fields, so one with none is refused. Declare at least one. A `store` field has a kind such as `Cell`, `Map` or `Set`. An agent with storage but no handler is `bynk.parse.empty_agent`.",
        example: "agent Counter {                          // ✗ no `store` field\n    key id: String\n\n    on call get() -> Effect[Int] { … }\n}\n\nagent Counter {\n    key id: String\n    store n: Cell[Int]                   // ✓\n\n    on call get() -> Effect[Int] { … }\n}",
        page: "/book/reference/agents",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.parse.expected_base_type",
        blurb: "After `type Name =` the parser found something that is not a record `{ … }`, a sum starting with `|`, an `enum` or an `opaque` type, so it wanted a base type (`Int`, `String`, `Bool`, `Float`, `Duration`, `Instant`, `Bytes` or `Decimal`) to build on. The commonest cause is a sum written without its leading `|`. Another is naming an existing record or sum, as in `type Id = Order`, which a type declaration cannot do; for a distinct type, use `opaque` over a base type. The position is the first token after the `=`.",
        example: "type Shape = Circle | Square              // ✗ a sum starts with `|`\ntype Shape = | Circle | Square            // ✓",
        page: "/book/reference/grammar",
        anchor: "rule-sum_type",
    },
    Explain {
        code: "bynk.parse.expected_capability_op",
        blurb: "A capability body is a list of operations, each a signature that starts with `fn`: the capability says what can be done, and a provider supplies how. The parser found something else where the next operation should begin. The commonest cause is leaving off the `fn`. An `on` handler, a `type` or a `let` does not belong in a capability either. The message names the token it found, and the position is where the operation should have started. A provider body has the same rule: `bynk.parse.expected_provider_op`.",
        example: "capability Clock {\n    now() -> Effect[Int]                  // ✗ an operation starts with `fn`\n}\n\ncapability Clock {\n    fn now() -> Effect[Int]               // ✓\n}",
        page: "/book/reference/grammar",
        anchor: "rule-capability_op",
    },
    Explain {
        code: "bynk.parse.expected_expression",
        blurb: "The parser needed an expression, a value, and found a token that cannot start one. The commonest cause is a record written without its type: `{ x: 0 }` is not an expression, so write `Point { x: 0 }`. Others are an operator with nothing after it (`a +`) and a trailing comma in a call (`f(a, )`). The message names the token it found, where the parser gave up. For an operator or a comma with nothing after it that is the token after the mistake, so read what comes before it.",
        example: "type Point = { x: Int, y: Int }\n\nfn origin() -> Point {\n    { x: 0, y: 0 }                        // ✗ a record literal needs its type's name\n}\n\nfn origin() -> Point {\n    Point { x: 0, y: 0 }                  // ✓\n}",
        page: "/book/reference/grammar",
        anchor: "rule-record_construction",
    },
    Explain {
        code: "bynk.parse.expected_handler",
        blurb: "A service or agent body is its handlers, each starting with `on`, beside its own clauses (a service's policies, an agent's `key` and `store` fields). The parser found something else where a handler should begin. The commonest cause is a helper `fn`, or a `let`, written inside the body: declare a helper beside the service, in the context, and its handlers can call it. Another is a handler with its `on` left off (`GET(\"/a\") …`, `call get() …`). The position is the first token of the item that is not a handler.",
        example: "service api from http {\n    fn double(n: Int) -> Int { n * 2 }    // ✗ only handlers go in a service\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}\n\nfn double(n: Int) -> Int { n * 2 }        // ✓ beside the service\n\nservice api from http {\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}",
        page: "/book/reference/grammar",
        anchor: "rule-handler",
    },
    Explain {
        code: "bynk.parse.expected_item",
        blurb: "A unit's body is a list of declarations, and the parser found something else where the next one should begin. The message lists what this kind of unit accepts (a commons, a context and a suite differ). The commonest cause is code at the top level, which holds declarations only: a `let` outside any function (a constant is `const Name: Type = …`) or a call. Others are a misspelt keyword (`function`, `pub fn`) and a stray `}` after a fragment-form unit's last declaration. The position is that token, so look just before it too.",
        example: "let limit = 100                           // ✗ code at the top level of a unit\nconst Limit: Int = 100                    // ✓ a named constant",
        page: "/book/reference/grammar",
        anchor: "rule-source_file",
    },
    Explain {
        code: "bynk.parse.expected_predicate",
        blurb: "After `where`, and after each `&&`, a refinement takes the name of a predicate from a fixed list (`NonNegative`, `InRange(0, 150)` and so on, which the note gives), not a comparison. The list is closed, so the compiler can check a literal against the refinement at compile time. The commonest cause is a comparison such as `where >= 0`; a name that is not on the list, as in `where n > 0`, is `bynk.parse.unknown_predicate`. The position is the token that is not a predicate name.",
        example: "type Age = Int where >= 0                 // ✗ a predicate is a name, not a comparison\ntype Age = Int where NonNegative          // ✓",
        page: "/book/reference/refined-types",
        anchor: "predicates",
    },
    Explain {
        code: "bynk.parse.expected_provider_op",
        blurb: "A provider body implements the operations its capability declares, each starting with `fn` and carrying a body. The parser found something else where the next operation should begin. The commonest cause is leaving off the `fn`; an `on` handler, a `type` or a `let` does not belong in a provider either. The message names the token it found, and the position is where the operation should have started. A capability body has the same rule: `bynk.parse.expected_capability_op`.",
        example: "capability Clock {\n    fn now() -> Effect[Int]\n}\n\nprovides Clock = FixedClock {\n    now() -> Effect[Int] { Effect.pure(1) }       // ✗ an operation starts with `fn`\n}\n\nprovides Clock = FixedClock {\n    fn now() -> Effect[Int] { Effect.pure(1) }    // ✓\n}",
        page: "/book/reference/grammar",
        anchor: "rule-provider_op",
    },
    Explain {
        code: "bynk.parse.expected_token",
        blurb: "The parser was part-way through a construct and the next token had to be one particular token: a `)` closing a list, a `,` between items, a `:`, `=`, `->` or `=>`, or a `}`. The message says which, and what it found instead. The commonest cause is a missing comma: in `fn add(a: Int b: Int)` it finds `b` where `)` was due. Others are `->` for `=>` in a match arm and `:` for `->` before a return type. A missing `)` or `}` is reported at the next token that cannot continue, often on the line below, so read the line above. If the file ended instead, it is `bynk.parse.unexpected_eof`.",
        example: "fn add(a: Int b: Int) -> Int { a + b }    // ✗ no `,` between the parameters\nfn add(a: Int, b: Int) -> Int { a + b }   // ✓",
        page: "/book/reference/grammar",
        anchor: "rule-fn_decl",
    },
    Explain {
        code: "bynk.parse.expected_type",
        blurb: "A type was expected here, and the next token cannot start one. The message says which position wanted it (`as the return type`, `as the parameter type`). The commonest cause is a type left out: a `->` with nothing before the `{`, or a `:` with nothing after it. A trailing comma in a type-argument list (`Map[String, ]`) and a value where a type belongs (`n: 5`) land here too. The position is the token after the gap, so look just before it.",
        example: "fn double(n: Int) -> {                    // ✗ no return type after `->`\n    n * 2\n}\n\nfn double(n: Int) -> Int {                // ✓\n    n * 2\n}",
        page: "/book/reference/grammar",
        anchor: "rule-_type_ref",
    },
    Explain {
        code: "bynk.parse.expected_visibility",
        blurb: "An `exports` clause says how consumers may use what it lists, so it needs a visibility: `exports opaque { … }` keeps a type's shape private to its context, `exports transparent { … }` shows it, and `exports capability { … }` offers a capability. A bare `exports { … }` is refused: the choice decides what other units can rely on, so it is written out. Pick one.",
        example: "exports { P }                           // ✗ no visibility\n\nexports transparent { P }               // ✓",
        page: "/book/reference/grammar",
        anchor: "rule-exports_decl",
    },
    Explain {
        code: "bynk.parse.exports_after_decls",
        blurb: "In the fragment form of a `context` (the header with no braces), \
                the `exports` clauses come after the `uses` and `consumes` lines \
                and before any `type` or `fn`, so what a unit offers is stated \
                at the top. Move the `exports` line up. `uses` and `consumes` \
                follow the same rule, as `bynk.parse.uses_after_decls` and \
                `bynk.parse.consumes_after_decls`.",
        example: "context shop.orders\n\
                  type Id = String\n\
                  exports transparent { Id }       // ✗ a declaration comes first\n\
                  \n\
                  context shop.orders\n\
                  exports transparent { Id }       // ✓ clauses straight after the header\n\
                  type Id = String",
        page: "/book/spec/syntactic-grammar",
        anchor: "",
    },
    Explain {
        code: "bynk.parse.extra_tokens",
        blurb: "A `.bynk` file holds one `commons` or `context`, and the parser found more after it ended. The commonest cause is a `}` too many: the unit closed early, or an extra brace sits at the end, and the first token after the close is reported, either the stray brace or the next declaration. Count the braces back from there. The other is a second `commons` or `context` in the same file; each goes in its own file. The position is the first token after the unit's end, not the brace that ended it. Inside a string, an interpolation `\\(…)` holds a single expression, and a second token in it lands here too.",
        example: "commons demo {\n    fn one() -> Int { 1 }\n}\n}                                         // ✗ nothing can follow the unit\n\ncommons demo {\n    fn one() -> Int { 1 }\n}                                         // ✓ the unit closes once",
        page: "/book/reference/grammar",
        anchor: "rule-source_file",
    },
    Explain {
        code: "bynk.parse.generic_arg_count",
        blurb: "`Result` takes two type arguments, the success type and the \
                error type: `Result[T, E]`. `Map` takes two as well, `Map[K, \
                V]`, and a type applied to an empty `[]` needs at least one. A \
                `Result[Int]` leaves out the error a caller has to handle, so it \
                is refused rather than guessed. Give every argument.",
        example: "fn parse(s: String) -> Result[Int] {            // ✗ the error type is missing\n\
                  \x20\x20\x20\x20Ok(1)\n\
                  }\n\
                  \n\
                  fn parse(s: String) -> Result[Int, String] {    // ✓\n\
                  \x20\x20\x20\x20Ok(1)\n\
                  }",
        page: "/book/reference/types",
        anchor: "built-in-generic-types",
    },
    Explain {
        code: "bynk.parse.handler_in_agent",
        blurb: "An `agent` has only `on call` handlers. HTTP routes (`on GET`, `on \
                POST`, …), schedules and queue messages belong on a `service`, \
                which receives the request or event and calls the agent. Move the \
                protocol handler to a service and give the agent an `on call` \
                operation for it to call.",
        example: "agent Tally {\n    \
                  key id: String\n    \
                  store count: Cell[Int]\n\n    \
                  on GET(\"/count\") () -> Effect[Result[Int, String]] {   // ✗ a route is a service's\n        \
                  Ok(count)\n    \
                  }\n\
                  }\n\n\
                  agent Tally {\n    \
                  key id: String\n    \
                  store count: Cell[Int]\n\n    \
                  on call get() -> Effect[Result[Int, String]] {         // ✓ the agent's own operation\n        \
                  Ok(count)\n    \
                  }\n\
                  }",
        page: "/book/reference/agents",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.parse.invariant_after_handler",
        blurb: "An agent's declarations come in phases: `store` fields, then \
                `invariant`s, then `on` handlers. An invariant after a handler \
                is refused, so that every property the handlers must preserve \
                sits together above them. Move the invariant above the first \
                handler.",
        example: "agent Tally {\n\
                  \x20\x20\x20\x20key id: String\n\
                  \x20\x20\x20\x20store count: Cell[Int]\n\
                  \n\
                  \x20\x20\x20\x20on call get() -> Effect[Result[Int, String]] {\n\
                  \x20\x20\x20\x20\x20\x20\x20\x20Ok(count)\n\
                  \x20\x20\x20\x20}\n\
                  \n\
                  \x20\x20\x20\x20invariant nonNegative: count >= 0                // ✗ after a handler\n\
                  }\n\
                  \n\
                  agent Tally {\n\
                  \x20\x20\x20\x20key id: String\n\
                  \x20\x20\x20\x20store count: Cell[Int]\n\
                  \n\
                  \x20\x20\x20\x20invariant nonNegative: count >= 0                // ✓ before the handlers\n\
                  \n\
                  \x20\x20\x20\x20on call get() -> Effect[Result[Int, String]] {\n\
                  \x20\x20\x20\x20\x20\x20\x20\x20Ok(count)\n\
                  \x20\x20\x20\x20}\n\
                  }",
        page: "/book/reference/agent-invariants",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.parse.keyword_field_shorthand",
        blurb: "A record pattern or construction may write a field by name alone \
                (`{ total }`), which binds a variable of that name. A reserved \
                word cannot be a variable, so `{ type, .. }` is refused, though \
                `type` is allowed as a field label before a `:`. In a pattern, \
                name the binding: `type: type_`; the compiler offers that fix. In \
                a construction, give the value: `type: \"Org\"`.",
        example: "match a {\n    \
                  Account { type, .. } => type             // ✗ shorthand binds a keyword\n    \
                  Account { type: type_, .. } => type_     // ✓ name the binding\n\
                  }",
        page: "/book/reference/types",
        anchor: "keyword-field-names",
    },
    Explain {
        code: "bynk.parse.lambda_pattern_annotation",
        blurb: "A lambda parameter written as a record pattern already names its \
                type, in `Item { … }`, and its names take their types from that \
                record, so a `: Type` after the pattern is refused. Remove the \
                annotation; the fix is machine-applicable, so `bynk check --fix` \
                applies it.",
        example: "items.map((Item { qty, .. }: Item) => qty)    // ✗ the pattern names the type\n\
                  items.map((Item { qty, .. }) => qty)          // ✓",
        page: "/book/spec/syntactic-grammar",
        anchor: "469b-lambda_param",
    },
    Explain {
        code: "bynk.parse.lambda_pattern_not_record",
        blurb: "A lambda parameter may be a pattern only when it is a record \
                pattern that cannot fail, as in `(Item { qty, .. }) => qty`. A \
                lambda is given a value and has no `else`, so a pattern that can \
                miss, such as `Some(n)`, a list, a literal or a range, has no \
                way to say what happens then. Take a name and `match` on it in \
                the body.",
        example: "xs.map((Some(n)) => n)               // ✗ a lambda has no `else` for `None`\n\
                  \n\
                  xs.map((o) => match o {              // ✓ the `match` covers both\n\
                  \x20\x20\x20\x20Some(n) => n\n\
                  \x20\x20\x20\x20None => 0\n\
                  })",
        page: "/book/spec/syntactic-grammar",
        anchor: "469b-lambda_param",
    },
    Explain {
        code: "bynk.parse.lambda_pattern_refutable",
        blurb: "A lambda parameter pattern must match every value of its type, \
                because a lambda has no `else` for the rest. `Item { status: \
                Paid, qty, .. }` matches only some items. Use bare names, `f: \
                name` or `f: _`; the note names the first field that can fail. \
                To handle the other cases, take a name and `match` on it in the \
                body.",
        example: "items.map((Item { status: Paid, qty, .. }) => qty)   // ✗ not every item is `Paid`\n\
                  \n\
                  items.map((i) => match i.status {                    // ✓ every status is handled\n\
                  \x20\x20\x20\x20Paid => i.qty\n\
                  \x20\x20\x20\x20Pending => 0\n\
                  })",
        page: "/book/spec/syntactic-grammar",
        anchor: "469b-lambda_param",
    },
    Explain {
        code: "bynk.parse.let_else_on_binding",
        blurb: "`else` after a `let` says what the block does when the pattern \
                does not match. A plain name matches every value, so this `else` \
                could never run, and it is refused rather than ignored. To \
                handle a failure, bind a pattern that can fail: `Ok(x)`, \
                `Some(x)`, or a qualified variant such as `Status.Pending`. \
                Otherwise drop the `else`.",
        example: "fn count(r: Result[Int, String]) -> Int {\n\
                  \x20\x20\x20\x20let n = r else { 0 }              // ✗ a name always matches\n\
                  \x20\x20\x20\x20n\n\
                  }\n\
                  \n\
                  fn count(r: Result[Int, String]) -> Int {\n\
                  \x20\x20\x20\x20let Ok(n) = r else { 0 }          // ✓ an `Err` takes the `else`\n\
                  \x20\x20\x20\x20n\n\
                  }",
        page: "/book/spec/syntactic-grammar",
        anchor: "484a-let_else_stmt-2010",
    },
    Explain {
        code: "bynk.parse.let_pattern_annotation",
        blurb: "A `let` that takes a value apart with a pattern gives its names \
                the types they have in the value, so it has no place for an \
                annotation after the pattern. Remove the `: Type`. To pin the \
                type of the value itself, annotate an ordinary `let` first and \
                match on that.",
        example: "fn count(r: Result[Int, String]) -> Int {\n\
                  \x20\x20\x20\x20let Ok(n): Int = r else { 0 }     // ✗ a pattern takes its types from the value\n\
                  \x20\x20\x20\x20n\n\
                  }\n\
                  \n\
                  fn count(r: Result[Int, String]) -> Int {\n\
                  \x20\x20\x20\x20let Ok(n) = r else { 0 }          // ✓\n\
                  \x20\x20\x20\x20n\n\
                  }",
        page: "/book/spec/syntactic-grammar",
        anchor: "484a-let_else_stmt-2010",
    },
    Explain {
        code: "bynk.parse.let_pattern_without_else",
        blurb: "A `let` that takes a value apart with a pattern must say what the \
                block produces when the value does not match, so it needs an \
                `else`. Only a record pattern that cannot fail (every field a bare \
                name, `f: name` or `f: _`) may leave it out. Add `else { … }` \
                after the value, or write a `match`. The note names the first \
                field that can fail to match.",
        example: "fn head(xs: List[Int]) -> Int {\n    \
                  let [x, ..] = xs               // ✗ an empty list does not match\n    \
                  x\n\
                  }\n\n\
                  fn head(xs: List[Int]) -> Int {\n    \
                  let [x, ..] = xs else { 0 }    // ✓ says what an empty list gives\n    \
                  x\n\
                  }",
        page: "/book/guides/type-system/destructure-a-record-with-let",
        anchor: "when-a-field-must-match",
    },
    Explain {
        code: "bynk.parse.list_rest_not_last",
        blurb: "In a list pattern the `..` (or `..rest`) stands for the elements \
                the pattern does not name, and it must be the last element and \
                appear once. A pattern that matches the end of a list, such as \
                `[.., last]`, is not available, so match from the front. When only \
                `_` patterns follow the `..`, the error offers a \
                machine-applicable fix that moves it to the end.",
        example: "match xs {\n    \
                  [.., a, b] => b    // ✗ `..` must come last\n    \
                  [_, b, ..] => b    // ✓ match from the front\n    \
                  _ => 0\n\
                  }",
        page: "/book/reference/types",
        anchor: "list-patterns",
    },
    Explain {
        code: "bynk.parse.malformed_float_literal",
        blurb: "A `Float` literal has a digit on each side of the point, as in \
                `1.0` and `0.5`; `1.` and `.5` are refused. This keeps a method \
                call on a number, such as `1.toFloat()` or `2.5.round()`, \
                unambiguous. Write the missing digit; the note gives the \
                corrected literal.",
        example: "const Half: Float = .5     // ✗ no digit before the point\n\
                  const Half: Float = 0.5    // ✓",
        page: "/book/spec/lexical-grammar",
        anchor: "321a-float_literal",
    },
    Explain {
        code: "bynk.parse.match_ends_block",
        blurb: "A block has no expression statements, so a `match` is its final \
                value and nothing can follow it; a second `match` straight after \
                the first is refused. Bind the first with `let _ = match …`. If \
                the steps are fallible and each depends on the one before, `let … \
                else` flattens them without nesting.",
        example: "fn grade(a: Int, b: Int) -> Int {\n    \
                  match a { … }\n    \
                  match b { … }              // ✗ a `match` ends its block\n\
                  }\n\n\
                  fn grade(a: Int, b: Int) -> Int {\n    \
                  let _ = match a { … }      // ✓ bound, so the block goes on\n    \
                  match b { … }\n\
                  }",
        page: "/book/guides/type-system/match",
        anchor: "flatten-a-staircase",
    },
    Explain {
        code: "bynk.parse.migrate_after_handler",
        blurb: "A `migrate` step carries a stored record from an older schema version to the agent's current shape, so it belongs with the agent's storage and invariants, above the handlers that read the migrated state. One written after a handler is refused. Move it above the first handler. `bynk.parse.invariant_after_handler` and `bynk.parse.transition_after_handler` are the same rule for invariants and transitions.",
        example: "type CartV1 = { owner: String, total: Int }\n\nagent Cart {\n    key id: String\n    store ownerName: Cell[String]\n    store totalCents: Cell[Int]\n\n    on call cents() -> Effect[Int] { … }\n\n    migrate from 1 (old: CartV1) {            // ✗ after a handler\n        ownerName := old.owner\n        totalCents := old.total * 100\n    }\n}\n\nagent Cart {\n    key id: String\n    store ownerName: Cell[String]\n    store totalCents: Cell[Int]\n\n    migrate from 1 (old: CartV1) {            // ✓ before the handlers\n        ownerName := old.owner\n        totalCents := old.total * 100\n    }\n\n    on call cents() -> Effect[Int] { … }\n}",
        page: "/book/reference/agents",
        anchor: "migrations",
    },
    Explain {
        code: "bynk.parse.migrate_item",
        blurb: "A `migrate` step carries a stored record from an older schema version to the agent's current shape, so its body is only a list of what happens to each field: `field := value` assigns a current field, `let x = value` binds a local for later lines, and `drop field` discards a stored field on purpose. A step is pure, so nothing else belongs in it. The commonest mistake is `=` where `:=` is meant; a bare expression lands here too. The position is the first token of the line that is none of the three.",
        example: "type CartV1 = { owner: String, total: Int }\n\nagent Cart {\n    key id: String\n    store ownerName: Cell[String]\n    store totalCents: Cell[Int]\n\n    migrate from 1 (old: CartV1) {\n        ownerName = old.owner                 // ✗ a step assigns with `:=`\n        totalCents := old.total * 100\n    }\n\n    on call cents() -> Effect[Int] { … }\n}\n\nagent Cart {\n    key id: String\n    store ownerName: Cell[String]\n    store totalCents: Cell[Int]\n\n    migrate from 1 (old: CartV1) {\n        ownerName := old.owner                // ✓\n        totalCents := old.total * 100\n    }\n\n    on call cents() -> Effect[Int] { … }\n}",
        page: "/book/reference/agents",
        anchor: "migrations",
    },
    Explain {
        code: "bynk.parse.multiple_docs",
        blurb: "A declaration or member has one doc. Two doc lines in a row are \
                two docs, not one of two lines. For more than one line, write a \
                doc block: a `---` line, the text, and another `---` line. When \
                every doc is a one-line `--- text`, the fix that joins them into \
                a block is machine-applicable, so `bynk check --fix` applies it.",
        example: "--- Doubles a number.\n\
                  --- Used by the pricing code.          // ✗ a second doc\n\
                  fn double(n: Int) -> Int { n * 2 }\n\
                  \n\
                  ---                                    // ✓ one doc block\n\
                  Doubles a number.\n\
                  Used by the pricing code.\n\
                  ---\n\
                  fn double(n: Int) -> Int { n * 2 }",
        page: "/book/spec/static-semantics",
        anchor: "one-doc-per-target",
    },
    Explain {
        code: "bynk.parse.no_remainder_operator",
        blurb: "Bynk has no `%` operator. Remainder is a method with its sign \
                spelled out: `a.rem(b)` takes the dividend's sign (the C, Java and \
                JavaScript `%`), `a.mod(b)` takes the divisor's (Python's `%`; use \
                it for a cyclic index). When both operands are plain names or \
                integer literals the error offers a `.rem` fix, never applied \
                automatically: check that the sign is the one you want.",
        example: "fn parity(n: Int) -> Int {\n    \
                  n % 2          // ✗ there is no `%`\n\
                  }\n\n\
                  fn parity(n: Int) -> Int {\n    \
                  n.rem(2)       // ✓ the dividend's sign, as `%` in C\n\
                  }\n\n\
                  fn slot(n: Int) -> Int {\n    \
                  n.mod(2)       // ✓ the divisor's sign, as `%` in Python\n\
                  }",
        page: "/book/reference/operators",
        anchor: "remainder",
    },
    Explain {
        code: "bynk.parse.non_associative",
        blurb: "Comparisons (`<`, `<=`, `>`, `>=`) and equality tests (`==`, `!=`) \
                do not chain, so `a < b < c` is refused rather than given a \
                reading you may not have meant. Split a chain of comparisons with \
                `&&`; to test the result of an equality, parenthesise it: `(a == \
                b) == c`.",
        example: "fn ordered(a: Int, b: Int, c: Int) -> Bool {\n    \
                  a < b < c         // ✗ comparisons do not chain\n\
                  }\n\n\
                  fn ordered(a: Int, b: Int, c: Int) -> Bool {\n    \
                  a < b && b < c    // ✓ one comparison on each side of `&&`\n\
                  }",
        page: "/book/reference/operators",
        anchor: "precedence",
    },
    Explain {
        code: "bynk.parse.policy_order",
        blurb: "A service's policies open its body, before any handler, in one fixed order: `cors`, `security`, `limits`, then `json`, each optional. One fixed layout means a reader always finds a service's cross-cutting rules at the top, and the formatter has a single form to write. A policy after a later one, or after a handler, is refused. Reorder them. A policy given twice is a separate error, such as `bynk.parse.duplicate_cors`.",
        example: "service api from http {\n    limits { maxBody: 1_048_576 }\n    cors { origins: [\"*\"] }               // ✗ `cors` comes before `limits`\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}\n\nservice api from http {\n    cors { origins: [\"*\"] }               // ✓ policies in order\n    limits { maxBody: 1_048_576 }\n    on GET(\"/a\") () -> Effect[HttpResult[Int]] by Visitor { … }\n}",
        page: "/book/reference/http",
        anchor: "cors",
    },
    Explain {
        code: "bynk.parse.record_pattern_in_is_unparenthesised",
        blurb: "After `is`, a record pattern is written in parentheses. Bare, \
                the `{` that opens its fields could equally open the block of \
                the `if`, and the reader could not tell which. Write `x is (Type \
                { field, .. })`. The fix is machine-applicable, so `bynk check \
                --fix` applies it. A `match` arm needs no parentheses.",
        example: "fn isPaid(o: Order) -> Bool {\n\
                  \x20\x20\x20\x20if o is Order { status: Paid, .. } { true } else { false }       // ✗ `{` is ambiguous\n\
                  }\n\
                  \n\
                  fn isPaid(o: Order) -> Bool {\n\
                  \x20\x20\x20\x20if o is (Order { status: Paid, .. }) { true } else { false }     // ✓\n\
                  }",
        page: "/book/spec/syntactic-grammar",
        anchor: "4711-record_pattern",
    },
    Explain {
        code: "bynk.parse.record_pattern_without_rest",
        blurb: "Every record pattern ends with `..`. Record patterns are always open: they \
                name the fields they care about and ignore the rest, so adding a field \
                to the record later never breaks a pattern that was already written. \
                The `..` makes that openness visible at the pattern. The compiler offers \
                the fix: it adds `..` after the last field.",
        example: "match o {\n    Order { total } => total          // ✗ no trailing `..`\n}\n\n\
                  match o {\n    Order { total, .. } => total      // ✓ open: other fields ignored\n}",
        page: "/book/reference/types",
        anchor: "record-patterns",
    },
    Explain {
        code: "bynk.parse.refined_pattern_inner",
        blurb: "A `where` guard on a `match` arm is written on the wildcard, as \
                `_ where InRange(0, 100)`. In this version no other pattern can \
                carry one, a name included, so binding the matched value \
                together with a guard is not yet supported. Write `_ where …`, \
                and refer to the value by the name it already has.",
        example: "match n {\n\
                  \x20\x20\x20\x20n where InRange(0, 100) => 1      // ✗ only `_` can take a `where`\n\
                  \x20\x20\x20\x20_ => 0\n\
                  }\n\
                  \n\
                  match n {\n\
                  \x20\x20\x20\x20_ where InRange(0, 100) => 1      // ✓\n\
                  \x20\x20\x20\x20_ => 0\n\
                  }",
        page: "/book/spec/syntactic-grammar",
        anchor: "4710-refined_pattern",
    },
    Explain {
        code: "bynk.parse.reserved_keyword",
        blurb: "A hard keyword (`type`, `fn`, `match`, `Int` and so on) was \
                written where a name is expected: a parameter, a `let` binding, an \
                operation name. Hard keywords are reserved everywhere, so none can \
                be a name. Rename it. A keyword is still allowed as a field label \
                when a `:` follows it (`{ type: String }`); the shorthand without \
                the `:` is `bynk.parse.keyword_field_shorthand`.",
        example: "fn describe(type: String) -> String {   // ✗ `type` is a reserved word\n    \
                  type\n\
                  }\n\n\
                  fn describe(kind: String) -> String {   // ✓ any other name\n    \
                  kind\n\
                  }",
        page: "/book/reference/keywords",
        anchor: "hard-keywords",
    },
    Explain {
        code: "bynk.parse.self_outside_method",
        blurb: "`self` is the receiver of a method, and only the first parameter \
                of a method declaration, written `fn Type.method(self, …)`, may be \
                named `self`. A free function has no receiver. Either name the \
                type the function belongs to, making it a method, or drop `self` \
                and take the value as an ordinary parameter.",
        example: "fn total(self) -> Int {          // ✗ a free function has no `self`\n    \
                  self.total\n\
                  }\n\n\
                  fn Order.total(self) -> Int {    // ✓ a method of `Order`\n    \
                  self.total\n\
                  }",
        page: "/book/spec/syntactic-grammar",
        anchor: "",
    },
    Explain {
        code: "bynk.parse.storage_after_phase",
        blurb: "An agent's body is ordered: its `key`, its storage fields (`state` and `store`), its invariants and migrations, then its handlers. Putting the fields first means the whole shape of the agent's persisted state is read before any code that uses it. A storage field declared after an invariant or a handler is refused. Move it up beside the other fields.",
        example: "agent Counter {\n    key id: String\n    store count: Cell[Int]\n\n    on call bump() -> Effect[()] { … }\n\n    store extra: Cell[Int] = 0            // ✗ storage after a handler\n}\n\nagent Counter {\n    key id: String\n    store count: Cell[Int]\n    store extra: Cell[Int] = 0            // ✓ with the other fields\n\n    on call bump() -> Effect[()] { … }\n}",
        page: "/book/reference/agents",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.parse.transition_after_handler",
        blurb: "A `transition` checks each committed step of an agent against the one before it, so it belongs with the agent's invariants, above the handlers whose steps it constrains. One written after a handler is refused. Move it above the first handler. `bynk.parse.invariant_after_handler` and `bynk.parse.migrate_after_handler` are the same rule for invariants and migrations.",
        example: "agent Counter {\n    key id: String\n    store n: Cell[Int]\n\n    on call get() -> Effect[Int] { … }\n\n    transition up:                        // ✗ after a handler\n        new.n >= old.n\n}\n\nagent Counter {\n    key id: String\n    store n: Cell[Int]\n\n    transition up:                        // ✓ before the handlers\n        new.n >= old.n\n\n    on call get() -> Effect[Int] { … }\n}",
        page: "/book/reference/agent-invariants",
        anchor: "step-invariants",
    },
    Explain {
        code: "bynk.parse.unexpected_eof",
        blurb: "The file ended while the parser still needed something: usually a closing `}`, `)` or `]`, or an expression after an operator or a `,`. The message says what it expected, and the position is the end of the file. The commonest cause is a `}` missing at the end of a braced unit or function. A missing brace with further declarations after it is reported as `bynk.parse.expected_token`, at the next `fn` or `type`, so it may be well above the end: check the nesting from the last declaration you edited.",
        example: "commons demo {\n    fn one() -> Int { 1 }                 // ✗ the file ends with the unit still open\n\ncommons demo {\n    fn one() -> Int { 1 }\n}                                         // ✓ closed",
        page: "/book/reference/grammar",
        anchor: "rule-commons_decl",
    },
    Explain {
        code: "bynk.parse.unknown_effect_method",
        blurb: "`Effect.pure(value)` is the only operation spelled on `Effect` \
                itself in an expression: it lifts a plain value into \
                `Effect[T]`. An effect otherwise comes from calling a handler or \
                a capability's operation. Use `Effect.pure`.",
        example: "fn lift(n: Int) -> Effect[Int] {\n\
                  \x20\x20\x20\x20Effect.of(n)                     // ✗ the only operation is `pure`\n\
                  }\n\
                  \n\
                  fn lift(n: Int) -> Effect[Int] {\n\
                  \x20\x20\x20\x20Effect.pure(n)                   // ✓\n\
                  }",
        page: "/book/spec/syntactic-grammar",
        anchor: "4619-effect_pure_expr",
    },
    Explain {
        code: "bynk.parse.unknown_handler_kind",
        blurb: "A handler is `on` and a kind the compiler knows: `call`, an HTTP \
                method, `schedule`, `message` and the like. An HTTP method is \
                written in capitals, `GET`, `POST`, `PUT`, `PATCH` or `DELETE`, \
                so `on get(…)` is not a handler. Spell the kind as one of these.",
        example: "service api from http {\n\
                  \x20\x20\x20\x20on get(\"/rows\") () -> Effect[HttpResult[List[Row]]] by v: Visitor { … }   // ✗ methods are capitals\n\
                  }\n\
                  \n\
                  service api from http {\n\
                  \x20\x20\x20\x20on GET(\"/rows\") () -> Effect[HttpResult[List[Row]]] by v: Visitor { … }   // ✓\n\
                  }",
        page: "/book/spec/syntactic-grammar",
        anchor: "442a-handler",
    },
    Explain {
        code: "bynk.parse.unknown_predicate",
        blurb: "A refined type is a base type plus a rule from a fixed set of \
                built-in predicates: `Matches`, `InRange`, `MinLength`, \
                `MaxLength`, `Length`, `NonNegative`, `Positive`, `NonEmpty` and \
                `IsoDate`. There are no user-defined predicates. Use one of these; \
                for a numeric range that is `InRange(lo, hi)`, inclusive at both \
                ends.",
        example: "type Age = Int where Between(0, 150)    // ✗ not a predicate\n\
                  type Age = Int where InRange(0, 150)    // ✓",
        page: "/book/spec/type-system",
        anchor: "63-refinement-predicates",
    },
    Explain {
        code: "bynk.parse.unknown_tier",
        blurb: "A test case's tier says how much of the program it runs against, and there are three: `unit`, `integration` and `system`. Any other name after `as` is refused, usually a misspelling. Leave the clause out for `unit`, the default.",
        example: "case \"doubles\" as weird {               // ✗ not a tier\n    expect double(2) == 4\n}\n\ncase \"doubles\" as unit {                // ✓\n    expect double(2) == 4\n}",
        page: "/book/reference/testing",
        anchor: "tiers-the-as-tier-clause",
    },
    Explain {
        code: "bynk.parse.uses_after_decls",
        blurb: "In the fragment form of a `commons` or `context` (the header with \
                no braces), the `uses` clauses come straight after the header, \
                before any `type` or `fn`. Move the `uses` line up. `consumes` and \
                `exports` clauses follow the same rule, as \
                `bynk.parse.consumes_after_decls` and \
                `bynk.parse.exports_after_decls`: `consumes` after the `uses` \
                lines, `exports` after the `consumes` lines.",
        example: "commons demo\n\
                  type Id = String\n\
                  uses util              // ✗ a declaration comes first\n\n\
                  commons demo\n\
                  uses util              // ✓ clauses straight after the header\n\
                  type Id = String",
        page: "/book/spec/syntactic-grammar",
        anchor: "",
    },
    Explain {
        code: "bynk.parse.variant_name_case",
        blurb: "A variant of a sum type or enum is written like a constant: an \
                uppercase letter, then letters, digits or `_` (`Active`, \
                `NotFound`). A lowercase variant such as `suspended` is refused. \
                Capitalise the name.",
        example: "type Status =\n    \
                  | Active\n    \
                  | suspended   // ✗ a variant starts with a capital\n\n\
                  type Status =\n    \
                  | Active\n    \
                  | Suspended   // ✓",
        page: "/book/reference/types",
        anchor: "sum-types",
    },
    Explain {
        code: "bynk.postgres.sql_invalid",
        blurb: "When the project has a `schema.sql` at its root, Bynk parses each literal SQL statement of a `Postgres` capability, so a syntax error shows up when you check, not on the first request. It also flags a statement of several commands, which a prepared statement does not accept: send each with its own call, or use `transaction`. The parser is a pure-Rust one with a Postgres dialect, not Postgres' own, so it can refuse valid SQL, a newer or rarer form; that is why this is a warning and never blocks a build. Nothing runs, and the statement is not checked against the schema, which Bynk does not own. Correct the statement, or delete `schema.sql` to turn the check off.",
        example: "let _ <- Main.all(\"SELEC id FROM orders\", [])        // ✗ expected a statement, found SELEC\n\nlet _ <- Main.all(\"SELECT id FROM orders\", [])      // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "postgres-sql-checked-for-syntax",
    },
    Explain {
        code: "bynk.postgres.sql_param_count",
        blurb: "When the project has a `schema.sql`, Bynk counts the parameters of each literal `Postgres` statement, by Postgres' rule: the highest `$n`, so `$2` alone still takes two values. It checks a list written out in place, or a name `let`-bound to one. A `$n` inside a string, a quoted name, a comment or a dollar-quoted body is text, not a parameter. Pass one `PgValue` for each, or change the SQL. A gap (`$2` with no `$1`) is counted as two parameters; Postgres refuses it at run time (it cannot type `$1`), which this check does not look for.",
        example: "let _ <- Main.all(\"SELECT id FROM orders WHERE id = $1 AND qty > $2\", [PgInteger(id)])                 // ✗ two parameters, one value\n\nlet _ <- Main.all(\"SELECT id FROM orders WHERE id = $1 AND qty > $2\", [PgInteger(id), PgInteger(5)])    // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "postgres-sql-checked-for-syntax",
    },
    Explain {
        code: "bynk.project.duplicate_module_doc",
        blurb: "A unit's module doc is one piece of prose. When a unit is split \
                across files, at most one of them may open with it: with two, the \
                tools would have to join them in some file order and the result \
                would read oddly. The error names every file that carries one. \
                Keep the doc in the file that best introduces the unit, and remove \
                the others.",
        example: "// text/case.bynk\n\
                  ---\n\
                  Text helpers.\n\
                  ---\n\
                  commons text\n\n\
                  // text/trim.bynk\n\
                  ---\n\
                  Trimming.\n\
                  ---\n\
                  commons text       // ✗ a second module doc\n\n\
                  commons text       // ✓ the doc stays in case.bynk",
        page: "/book/guides/program-structure/document-your-code",
        anchor: "one-module-doc-per-unit",
    },
    Explain {
        code: "bynk.project.export_context",
        blurb: "A package's `exports` list names the commons and adapters it shares. A context is a deployment unit, owned by the project that deploys it, so a package never shares one, and listing one is refused. Remove it, or move what other projects need into a commons and list that. A name that is no unit of the package at all is `bynk.project.export_unknown`.",
        example: "[package]\nname = \"acme/shop\"\nexports = [\"shop.checkout\"]     // ✗ a context\n\nexports = [\"money\"]             // ✓ a commons",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "make-a-package",
    },
    Explain {
        code: "bynk.project.inconsistent_commons_name",
        blurb: "A source file's path is its unit's identity: `context \
                commerce.orders` lives at `commerce/orders.bynk`, or in a file \
                under `commerce/orders/`, relative to an include root. A reader \
                can then find a unit from its name alone. Rename the file or \
                directory to match the declared name, or change the name. Test \
                suites are exempt.",
        example: "// pricing.bynk\n\
                  commons tax     // ✗ the path says `pricing`\n\n\
                  // tax.bynk\n\
                  commons tax     // ✓",
        page: "/book/guides/projects-build-and-deployment/layout",
        anchor: "path-identity",
    },
    Explain {
        code: "bynk.project.kind_conflict",
        blurb: "A name belongs to one unit, so every file declaring it must agree \
                on its kind. One file saying `commons shop` and another `context \
                shop` would make the name both a library and a deployable unit. \
                Give the files different names, or make them the same kind. A \
                context is the deployable unit, and a commons is the shared code \
                it uses.",
        example: "// shop/types.bynk\n\
                  commons shop        // ✗ the name is also a context\n\
                  commons shop.types  // ✓\n\n\
                  // shop/api.bynk\n\
                  context shop",
        page: "/book/spec/compilation-model",
        anchor: "83-source-layout-and-test-ness",
    },
    Explain {
        code: "bynk.property.where_not_bool",
        blurb: "A `where` clause on `for all` discards each generated input \
                that fails it, so it must be a `Bool`. A filter of another \
                type, such as an `Int`, cannot say whether to keep an input. \
                Write a comparison over the bound variables.",
        example: "for all n: Int where n + 1 {   // ✗ an `Int`, not a `Bool`\n\n\
                  for all n: Int where n > 0 {   // ✓",
        page: "/book/reference/testing",
        anchor: "properties",
    },
    Explain {
        code: "bynk.provider.agent_call_undeclared_capability",
        blurb: "A provider calls an agent handler, and the handler needs a capability \
                the provider's `given` does not list. A handler's `given` is \
                widened to cover the agent handlers it calls, but a provider's is \
                not. The provider is constructed with exactly the capabilities in \
                its `given`, and those are all it can hand on to the agent. So the \
                provider must list everything the agent handler needs: the \
                handler's own `given`, and the `given` of any agent handler it \
                calls in turn.",
        example: "agent Index {\n    \
                    key name: String\n    \
                    store hits: Cell[Int]\n    \
                    on call withdraw() -> Effect[()] given Files { … }\n}\n\n\
                  provides Takedown = Bucket given Clock {\n    \
                    fn take(name: String) -> Effect[()] { Index(name).withdraw() }   // ✗ `withdraw` needs `Files`\n}\n\n\
                  provides Takedown = Bucket given Clock, Files {  // ✓ `Files` is listed\n    \
                    fn take(name: String) -> Effect[()] { Index(name).withdraw() }\n}",
        page: "/book/guides/effects-and-capabilities/compose-a-provider",
        anchor: "the-rules",
    },
    Explain {
        code: "bynk.provider.dependency_cycle",
        blurb: "A provider's `given` lists the capabilities it is built from, and the context builds providers in dependency order. A capability that depends on itself, directly or through other providers, has no order to be built in, so every provider on the cycle is reported. Break the cycle by removing the `given`, or by depending on a different capability. Dependencies on other contexts follow `consumes`, which has its own check, `bynk.context.consumes_cycle`.",
        example: "capability Clock {\n    fn now() -> Effect[Int]\n}\n\nprovides Clock = Fixed given Clock {   // ✗ `Clock` is built from itself\n    fn now() -> Effect[Int] { Effect.pure(0) }\n}\n\nprovides Clock = Fixed {               // ✓ built from nothing\n    fn now() -> Effect[Int] { Effect.pure(0) }\n}",
        page: "/book/guides/effects-and-capabilities/compose-a-provider",
        anchor: "the-rules",
    },
    Explain {
        code: "bynk.provider.extra_operation",
        blurb: "A `provides` block offers the capability's operations and no others. Callers reach a provider only through the capability, so an operation it does not declare could never be called, and it usually means a misspelt name or an operation added to the wrong capability. Remove it, or declare it on the capability. A capability operation the provider omits is `bynk.provider.missing_operation`.",
        example: "capability Clock {\n    fn now() -> Effect[Int]\n}\n\nprovides Clock = Fixed {\n    fn now() -> Effect[Int] { Effect.pure(0) }\n    fn zone() -> Effect[String] { Effect.pure(\"UTC\") }   // ✗ `Clock` declares no `zone`\n}\n\ncapability Clock {\n    fn now() -> Effect[Int]\n    fn zone() -> Effect[String]  // ✓ declared on the capability\n}",
        page: "/book/reference/capabilities",
        anchor: "providing-a-capability",
    },
    Explain {
        code: "bynk.provider.generic_op_requires_external",
        blurb: "A capability operation with its own type parameter, `fn dedup[T](…)`, can only be implemented by an external provider: a class written in TypeScript in an adapter's binding file. A provider with a Bynk body has no `T` to work with, so it could only return a value it was already given. Declare the capability in an `adapter` and give `provides` no body. An external provider inside a context is refused, as `bynk.context.external_provider`.",
        example: "capability Audit {\n    fn note[T](key: String) -> Effect[()]\n}\n\nprovides Audit = MemoAudit {   // ✗ a generic operation cannot have a Bynk body\n    fn note(key: String) -> Effect[()] { … }\n}\n\nadapter audit {\n    binding \"./audit.binding.ts\"\n    provides Audit = MemoAudit   // ✓ the binding supplies it\n}",
        page: "/book/reference/capabilities",
        anchor: "generic-operations",
    },
    Explain {
        code: "bynk.provider.missing_operation",
        blurb: "A `provides` block implements a capability, so it must supply every operation the capability declares: callers are checked against the capability, and an operation the provider lacks would have nothing to run. The error is on the `provides` line and names the missing operation. Add it, with the signature the capability declares. An operation the capability does not declare is `bynk.provider.extra_operation`.",
        example: "capability Clock {\n    fn now() -> Effect[Int]\n    fn zone() -> Effect[String]\n}\n\nprovides Clock = Fixed {         // ✗ no `zone`\n    fn now() -> Effect[Int] { Effect.pure(0) }\n}\n\nprovides Clock = Fixed {         // ✓ every operation is provided\n    fn now() -> Effect[Int] { Effect.pure(0) }\n    fn zone() -> Effect[String] { Effect.pure(\"UTC\") }\n}",
        page: "/book/reference/capabilities",
        anchor: "providing-a-capability",
    },
    Explain {
        code: "bynk.provider.outside_context",
        blurb: "A `provides` block says how a context's own capability is implemented, so it belongs in the context that declares the capability. A commons holds only pure declarations, with no effects to provide. Move the provider into the context. A `capability` in a commons is refused for the same reason, as `bynk.capability.outside_context`.",
        example: "commons demo\n\nprovides Clock = FixedClock {   // ✗ a commons cannot provide a capability\n    fn now() -> Effect[Int] { … }\n}\n\ncontext demo\n\ncapability Clock { fn now() -> Effect[Int] }\n\nprovides Clock = FixedClock {   // ✓ in the context that declares `Clock`\n    fn now() -> Effect[Int] { … }\n}",
        page: "/book/reference/capabilities",
        anchor: "providing-a-capability",
    },
    Explain {
        code: "bynk.provider.signature_mismatch",
        blurb: "A provider operation must have the signature its capability declares: the same number of parameters, the same parameter types and the same return type. Callers are checked against the capability, so a provider that returns something else would hand them a value of the wrong type. The error points at the parameter or return type that differs, or at the operation when the counts differ. Change the provider to match, or change the capability, and with it every provider of it.",
        example: "capability Clock {\n    fn now() -> Effect[Int]\n}\n\nprovides Clock = Fixed {\n    fn now() -> Effect[String] { Effect.pure(\"0\") }   // ✗ the capability returns `Effect[Int]`\n}\n\nprovides Clock = Fixed {\n    fn now() -> Effect[Int] { Effect.pure(0) }        // ✓ same signature\n}",
        page: "/book/reference/capabilities",
        anchor: "providing-a-capability",
    },
    Explain {
        code: "bynk.provider.unknown_capability",
        blurb: "A `provides` block implements a capability that the same context declares, and none of that name is declared here, so there is nothing to check the operations against. This is usually a misspelt name, or a capability that lives in another context: a provider cannot implement one from elsewhere. Declare the capability next to its provider, or correct the name.",
        example: "provides Clock = Fixed {         // ✗ this context declares no `Clock`\n    fn now() -> Effect[Int] { Effect.pure(0) }\n}\n\ncapability Clock {               // ✓ declared alongside its provider\n    fn now() -> Effect[Int]\n}",
        page: "/book/reference/capabilities",
        anchor: "providing-a-capability",
    },
    Explain {
        code: "bynk.query.join_key_mismatch",
        blurb: "`joinOn` and `leftJoin` pair up rows whose two keys are equal, and equality is only meaningful between values of one type. A `String` key on one side and an `Int` on the other can never match, so the join is refused. Change one key function so that both return the same type. A join on a condition rather than equal keys is `join`.",
        example: "type Order = { customerId: String, region: Int }\ntype Customer = { code: String, region: Int }\n\norders.joinOn(customers, (o) => o.customerId, (c) => c.region, (o, c) => c.region).count()   // ✗ `String` against `Int`\n\norders.joinOn(customers, (o) => o.customerId, (c) => c.code, (o, c) => c.region).count()     // ✓ both `String`",
        page: "/book/reference/types",
        anchor: "joins-and-grouping",
    },
    Explain {
        code: "bynk.query.sum_needs_numeric",
        blurb: "`sum` and `average` add the values the key function returns, so the key must be numeric: `Int`, `Float` or `Duration`, or a refined type over one of them. A `String` or a record cannot be added. Return a numeric field. `sum` also adds a `Decimal` key, exactly, on a `List` and on a `Query`. `average` does not take one: a mean is a division, and a `Decimal` division needs a scale and a rounding mode, so sum the key, then `div(Decimal.fromInt(n), scale, mode)` by the count. To pick by an ordering rather than add, `min`, `max` and `sortBy` accept any orderable key.",
        example: "type Line = { sku: String, qty: Int }\n\nfn total(lines: List[Line]) -> Int {\n    let t = lines.sum((l) => l.sku)   // ✗ a `String` cannot be summed\n    0\n}\n\nfn total(lines: List[Line]) -> Int {\n    lines.sum((l) => l.qty)           // ✓ a numeric key\n}",
        page: "/book/reference/types",
        anchor: "query",
    },
    Explain {
        code: "bynk.queue.bad_params",
        blurb: "A queue consumer is invoked once per message, so `on message` takes exactly one parameter: the message, of any type that can be decoded from the queue body. Two parameters leave the second with nothing to supply it, and none leaves the handler without the message. Put what you need into the message type itself: a record with those fields is one parameter. Name the parameter `message` by convention.",
        example: "type Msg = { a: String }\n\non message(message: Msg, extra: String) -> Effect[QueueResult] { … }   // ✗ two parameters\n\non message(message: Msg) -> Effect[QueueResult] { … }   // ✓ one message",
        page: "/book/reference/queue",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.queue.duplicate_consumer",
        blurb: "The context's `queue` entry point picks the handler by the queue's name. Two `on message` handlers for the same name would leave it with two to run. Merge them into one handler, or move one to a `from queue` service for another queue.",
        example: "service worker from queue(\"jobs\") {\n    on message(message: String) -> Effect[QueueResult] { … }\n    on message(message: String) -> Effect[QueueResult] { … }   // ✗ `jobs` already has a consumer\n}\n\nservice audit from queue(\"audit\") {\n    on message(message: String) -> Effect[QueueResult] { … }   // ✓ a queue of its own\n}",
        page: "/book/reference/queue",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.queue.invalid_name",
        blurb: "`from queue(\"…\")` names a Cloudflare queue, so the name must be one Cloudflare accepts: 1 to 63 lowercase letters, digits or `-`, starting and ending with a letter or digit. An empty name, capitals or `_` are refused here, rather than failing at deploy. Rename it, for example `order-events`.",
        example: "service worker from queue(\"Order_Events\") {\n    on message(message: String) -> Effect[QueueResult] { … }   // ✗ capitals and `_` in the name\n}\n\nservice worker from queue(\"order-events\") {\n    on message(message: String) -> Effect[QueueResult] { … }   // ✓\n}",
        page: "/book/reference/queue",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.queue.return_not_queue_result",
        blurb: "A queue consumer answers the queue, not a caller, and the only answers it understands are `Ack` (the message is handled) and `Retry(reason)` (deliver it again). The handler's return type is therefore fixed at `Effect[QueueResult]`, so a `Result` or a value cannot be mistaken for a verdict. Return `Ack` on success and `Retry` with a reason when the message should come back. The cron counterpart is `bynk.cron.return_not_effect_result`.",
        example: "on message(message: String) -> Effect[Result[Int, String]] { … }   // ✗ the queue needs a verdict\n\non message(message: String) -> Effect[QueueResult] { … }   // ✓ `Ack` or `Retry(…)`",
        page: "/book/reference/queue",
        anchor: "handler-form",
    },
    Explain {
        code: "bynk.race.in_agent_handler",
        blurb: "`race` hands the side that loses to the execution context, and an agent handler has none. Call it from the service handler that calls the agent, so the race is run from outside. The same rule holds for `bynk.within.in_agent_handler`.",
        example: "agent Feed {\n    key id: String\n\n    on call quote() -> Effect[Int] {\n        Slow(\"a\").get().race(Slow(\"b\").get())   // ✗ an agent handler has no execution context\n    }\n}\n\nservice quotes {\n    on call() -> Effect[Int] {\n        Slow(\"a\").get().race(Slow(\"b\").get())   // ✓ the service handler has\n    }\n}",
        page: "/book/reference/capabilities",
        anchor: "race",
    },
    Explain {
        code: "bynk.race.in_pure_context",
        blurb: "`race` starts two effects and hands the loser to the execution context, and only an effectful body has one: a pure function never runs an effect at all. Change the enclosing function's return type to `Effect[…]` so its body can run it. The same rule holds for `bynk.within.in_pure_context` and `bynk.send.in_pure_context`.",
        example: "fn first(a: Effect[Int], b: Effect[Int]) -> Int {\n    let c = a.race(b)   // ✗ the body is pure\n    0\n}\n\nfn first(a: Effect[Int], b: Effect[Int]) -> Effect[Int] {   // ✓ an effectful body\n    a.race(b)\n}",
        page: "/book/reference/capabilities",
        anchor: "race",
    },
    Explain {
        code: "bynk.race.outside_handler",
        blurb: "`race` hands the side that loses to the execution context, which only a service handler's body, or a function's it calls, has. A WebSocket handler, a provider operation and a test `case` have none. Move the `race` into a service handler or a function, and have the case call that, stubbing a slow side with `stub … after`. In an agent handler the rule is `bynk.race.in_agent_handler`.",
        example: "// tests/t.test.bynk\nlet r <- Slow(\"a\").get().race(Slow(\"b\").get())   // ✗ a case has no execution context\n\nlet r <- quotes.call()                           // ✓ the service handler races",
        page: "/book/reference/capabilities",
        anchor: "race",
    },
    Explain {
        code: "bynk.record_spread.field_type_mismatch",
        blurb: "An override in a record spread must have the type its field declares. The spread produces a value of the record's type, and a field of another type would break that type, so the mismatch is refused at the override instead of surfacing later. Give the field a value of its declared type, converting explicitly where needed: nothing converts implicitly.",
        example: "type Counter = { n: Int }\n\nfn bump(c: Counter) -> Counter {\n    Counter { ...c, n: \"no\" }   // ✗ `n` is an `Int`\n}\n\nfn bump(c: Counter) -> Counter {\n    Counter { ...c, n: 1 }   // ✓ an `Int`\n}",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.record_spread.non_record_base",
        blurb: "The base of a spread supplies the fields that are not overridden, so it has to be a value of a record type. An `Int`, a `List` or a sum value has no fields to copy. Spread a record, or build the record with every field written out. A base of a different record is `bynk.record_spread.type_mismatch`.",
        example: "type Counter = { n: Int }\n\nfn bump(count: Int, c: Counter) -> Counter {\n    Counter { ...count, n: 1 }   // ✗ `count` is an `Int`, not a record\n}\n\nfn bump(count: Int, c: Counter) -> Counter {\n    Counter { ...c, n: count }   // ✓ spread the record, override the field\n}",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.record_spread.type_mismatch",
        blurb: "A spread, `T { ...base, x: 1 }`, copies the fields of `base` that you do not name, so the result has the base's own type. It cannot convert one record type to another, even one with the same fields, and a `T` that names a different type from the base's is refused. Write the base's type, or, to build a different record, give every field: `Point { x: s.x, y: s.y }`. A base that is not a record is `bynk.record_spread.non_record_base`.",
        example: "type Point = { x: Int, y: Int }\ntype Size = { x: Int, y: Int }\n\nfn widen(s: Size) -> Size { Point { ...s, x: 1 } }   // ✗ `s` is a `Size`, not a `Point`\n\nfn widen(s: Size) -> Size { Size { ...s, x: 1 } }    // ✓ the base's own type",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.record_spread.unknown_field",
        blurb: "A record spread, `T { ...base, field: value }`, copies every field of `base` and replaces the ones you list. Each listed field has to be one the record declares: a misspelt or invented name would otherwise be dropped silently, or build a value that is not a `T`. Check the field name against the type's declaration. To add a field, declare it on the type first.",
        example: "type Counter = { n: Int }\n\nfn bump(c: Counter) -> Counter {\n    Counter { ...c, missing: 0 }   // ✗ `Counter` has no field `missing`\n}\n\nfn bump(c: Counter) -> Counter {\n    Counter { ...c, n: 0 }   // ✓ an existing field\n}",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.refine.literal_violates",
        blurb: "A literal written where a refined type is expected is checked at compile \
                time, and this one does not satisfy the type's predicate. A refined \
                type is a base type plus a rule (`InRange`, `NonEmpty`, …), and a \
                literal that breaks the rule can never be a valid value, so the \
                compiler rejects it instead of admitting it. Use a value that \
                satisfies the predicate, or loosen the type. A value known only at \
                run time goes through `.of`, which returns a `Result`.",
        example: "type Age = Int where InRange(0, 150)\n\n\
                  fn limit() -> Age { 200 }   // ✗ 200 is outside InRange(0, 150)\n\
                  fn limit() -> Age { 150 }   // ✓ admitted at compile time",
        page: "/book/reference/refined-types",
        anchor: "literal-admission",
    },
    Explain {
        code: "bynk.registry.withdrawn",
        blurb: "Your `bynk.lock` locks a version that its registry has withdrawn. A \
                withdrawal is a takedown — for a legal request or confirmed malware — \
                so unlike a yank, which keeps a version for locked builds, the \
                registry has deleted its archive and there is nothing to fetch. The \
                message quotes the registry's reason. A machine that already has the \
                tree in its store keeps building; elsewhere, move off the version \
                with `bynk update` and the dependency's alias. The reason is unsigned, so when it was \
                read through a mirror the message says the origin wasn't consulted.",
        example: "error[bynk.registry.withdrawn]: `acme/money` 1.4.2 was withdrawn by \
                  registry `acme-internal`: \"removed at the copyright holder's request\"\n  \
                  = note: `bynk.lock` locks this version; choose another with `bynk update money`\n\n\
                  $ bynk update money     # resolves to the newest version not withdrawn",
        page: "/book/guides/projects-build-and-deployment/run-a-private-registry",
        anchor: "withdraw-a-version",
    },
    Explain {
        code: "bynk.requires.conflicting_range",
        blurb: "A project installs one version of each npm package, so every adapter that `requires` a package must give the same range for it. Two adapters that ask for `jose` at `^4` and `^5` leave the build with no version that satisfies both, so the second is refused and names the first. Give both the same range. A package the toolchain itself pins, such as the one its own bindings use, must be required at that exact version.",
        example: "// auth.bynk\nbinding \"./auth.binding.ts\" requires { \"jose\": \"^5\" }\n\n// tokens.bynk\nbinding \"./tokens.binding.ts\" requires { \"jose\": \"^4\" }   // ✗ `auth` already requires `^5`\n\nbinding \"./tokens.binding.ts\" requires { \"jose\": \"^5\" }   // ✓ one range",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "a-dependency-that-wraps-an-npm-library",
    },
    Explain {
        code: "bynk.requires.unapproved",
        blurb: "An adapter in a git or registry dependency requires an npm package \
                your project hasn't approved. A dependency's adapters bring their \
                npm packages into your build's `package.json`, and the next \
                `npm install` runs whatever those packages run; so an npm package \
                from someone else's code enters only after you approve it once. \
                The approval is one package at one range, recorded in `bynk.lock` \
                (`approved = true`), so a changed range asks again. Your own \
                adapters and path dependencies need no approval.",
        example: "# bynk.lock, as a fetch wrote it — the build fails until approved\n\
                  [[npm]]\npackage = \"jose\"\nrange = \"^5.2.0\"\n\
                  required-by = [\"acme/money:tokens\"]\n\n\
                  # after `bynk approve jose` — the build passes\n\
                  [[npm]]\npackage = \"jose\"\nrange = \"^5.2.0\"\n\
                  required-by = [\"acme/money:tokens\"]\napproved = true",
        page: "/book/guides/projects-build-and-deployment/share-code-between-projects",
        anchor: "approve-a-dependencys-npm-packages",
    },
    Explain {
        code: "bynk.requires.unpinned_dependency",
        blurb: "An adapter's `requires` states the npm versions its binding is written against, and the build installs exactly what it says. `*`, `x`, `latest` and a range with no digit in it accept whatever version is newest on the day, so two builds of the same source can differ. Name a range that has a version in it, such as `^5.2.0`.",
        example: "binding \"./tokens.binding.ts\" requires { \"jose\": \"*\" }        // ✗ any version at all\n\nbinding \"./tokens.binding.ts\" requires { \"jose\": \"^5.2.0\" }   // ✓ a pinned range",
        page: "/book/reference/adapters",
        anchor: "anatomy",
    },
    Explain {
        code: "bynk.resolve.ambiguous_variant",
        blurb: "A bare variant name such as `Failed(\"down\")` builds a value of the sum that declares it. When two sums in scope declare the name (two of your own, or two that a consumed unit exports, like an adapter's error enums), the compiler reads the expected type: an annotated `let`, a parameter, a return type, or a `stub`'s operation. It builds the sum that type names. With nothing expecting either sum it cannot know which you meant, and it never picks one, so the name is refused. Qualify it as `TypeName.Variant`, or give the value a type. A `match` arm needs neither, because it reads its scrutinee's sum.",
        example: "type DbError = | Constraint(message: String) | Failed(message: String)\n\
                  type MailError = | RateLimited | Failed(message: String)\n\n\
                  let e = Failed(\"down\")       // ✗ `DbError` or `MailError`?\n\n\
                  fn mail() -> Result[Int, MailError] {\n\
                  \x20   Err(Failed(\"down\"))          // ✓ the return type picks `MailError`\n\
                  }\n\n\
                  DbError.Failed(\"down\")       // ✓ qualified",
        page: "/book/reference/types",
        anchor: "shared-variant-names",
    },
    Explain {
        code: "bynk.resolve.arity_mismatch",
        blurb: "A call to a declared `fn` must pass exactly as many arguments as it has parameters. Bynk has no optional or variadic parameters and fills nothing in, so a missing or extra argument is always a mistake, and the compiler cannot tell which one was meant. Add or remove the argument, or change the declaration. The error points at the call and labels the declaration. A call through a function value or a built-in operation is `bynk.types.call_arity`, and a method is `bynk.types.method_arity`.",
        example: "fn double(x: Int) -> Int {\n    x * 2\n}\n\nfn run(a: Int, b: Int) -> Int {\n    double(a, b)             // ✗ `double` takes one argument\n}\n\nfn run(a: Int, b: Int) -> Int {\n    double(a) + double(b)    // ✓ one argument each call\n}",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.bad_const_initialiser",
        blurb: "A constant's value is fixed when its module loads, so its initialiser must be static: a literal, a `Duration` literal, a record, list or variant built from static values, `Some`/`None`/`Ok`/`Err`, `T.unsafe(lit)`, an empty `List`/`Map`/`Set`, or another constant. A function call (even a pure one), an operator, an interpolated string, a field access, a lambda, `if` and `match` are not. The code is also reported when a static value is not of the declared type. Write the value out, or make it a `fn` and call it where it is used.",
        example: "fn base() -> Int {\n  10\n}\n\nconst Limit: Int = base()   // ✗ a call is not a static value\n\nconst Limit: Int = 10   // ✓",
        page: "/book/reference/constants",
        anchor: "what-an-initialiser-may-be",
    },
    Explain {
        code: "bynk.resolve.const_cycle",
        blurb: "Constants may refer to each other in any order, across files and through `uses`, but a value cannot depend on itself: in a cycle no member has anything to start from. The error lists the cycle (`First` → `Second` → `First`) and is reported once, at the member declared first. Break the cycle by giving one member a literal or other static value. A reference such as `Second` is itself a static initialiser, so this is the only rule that limits constants naming each other; `bynk.resolve.bad_const_initialiser` covers initialisers that are not static at all.",
        example: "const First: Int = Second   // ✗ `First` → `Second` → `First`\nconst Second: Int = First\n\nconst Second: Int = 10   // ✓ end the chain at a literal",
        page: "/book/reference/constants",
        anchor: "what-an-initialiser-may-be",
    },
    Explain {
        code: "bynk.resolve.const_name_case",
        blurb: "A constant is named in upper-camel case, a capital letter and then letters, digits or `_`, like a variant, so that a constant reads differently from a binding wherever it is used. The grammar accepts any identifier here and leaves the case to the checker, so a lower-case name is reported with a suggested spelling instead of failing to parse. Rename it, and every use of it: `MaxRetries`, not `maxRetries`.",
        example: "const maxRetries: Int = 3   // ✗ a constant's name starts with a capital\n\nconst MaxRetries: Int = 3   // ✓",
        page: "/book/reference/constants",
        anchor: "declaring-a-constant",
    },
    Explain {
        code: "bynk.resolve.const_outside_context_or_commons",
        blurb: "An adapter is the program's boundary with the host: it declares capabilities and the providers a binding supplies, not values for the program to share, so a `const` is refused there. Declare the constant in a commons, which a unit reads by `uses`, or in the context that needs it, and move it out of the adapter.",
        example: "// tokens.bynk\nconst Lifetime: Int = 3600     // ✗ an adapter declares no constants\n\n// limits.bynk\ncommons limits\nconst Lifetime: Int = 3600     // ✓ in a commons",
        page: "/book/reference/constants",
        anchor: "declaring-a-constant",
    },
    Explain {
        code: "bynk.resolve.duplicate_actor",
        blurb: "An actor is named by the `by` clause of a handler, and its name is one of the unit's top-level names, so two actors of one name leave a `by` with nothing to choose between. Rename one, or delete the repeat. The same holds for the other named declarations: `bynk.resolve.duplicate_service`, `bynk.resolve.duplicate_agent` and `bynk.resolve.duplicate_capability`.",
        example: "actor Admin { auth = None }\nactor Admin { auth = None }      // ✗ `Admin` is already declared\n\nactor Admin { auth = None }\nactor Support { auth = None }    // ✓ a name of its own",
        page: "/book/reference/actors",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.resolve.duplicate_agent",
        blurb: "An agent is addressed by its name, and the name is one of the unit's top-level names, so two agents of one name leave a call with nothing to choose between. Rename one, or delete the repeat. The same holds for the other named declarations: `bynk.resolve.duplicate_service`, `bynk.resolve.duplicate_actor` and `bynk.resolve.duplicate_capability`.",
        example: "agent Counter { … }\nagent Counter { … }    // ✗ `Counter` is already declared\n\nagent Counter { … }\nagent Tally { … }      // ✓ a name of its own",
        page: "/book/reference/agents",
        anchor: "declaration",
    },
    Explain {
        code: "bynk.resolve.duplicate_capability",
        blurb: "A capability is named by the `given` clauses that ask for it and by the calls made through it, and its name is one of the unit's top-level names, so two capabilities of one name leave those with nothing to choose between. Rename one, or delete the repeat. The same holds for the other named declarations: `bynk.resolve.duplicate_service`, `bynk.resolve.duplicate_agent` and `bynk.resolve.duplicate_actor`. A capability provided twice is `bynk.resolve.duplicate_provider`.",
        example: "capability Clock { … }\ncapability Clock { … }       // ✗ `Clock` is already declared\n\ncapability Clock { … }\ncapability Calendar { … }    // ✓ a name of its own",
        page: "/book/reference/capabilities",
        anchor: "declaring-a-capability",
    },
    Explain {
        code: "bynk.resolve.duplicate_const",
        blurb: "A constant is a value that every use of its name reads, so a name is declared once in a unit. With two `Limit`s, a use would have two values to read. Rename one, or delete the repeat. A constant whose name is a function's, a type's or another declaration's in the unit is `bynk.resolve.name_conflict`.",
        example: "const Limit: Int = 1\nconst Limit: Int = 2   // ✗ `Limit` is already declared\n\nconst Cap: Int = 2     // ✓ a different name",
        page: "/book/reference/constants",
        anchor: "declaring-a-constant",
    },
    Explain {
        code: "bynk.resolve.duplicate_field",
        blurb: "A record is built and read by field name, so each name may appear once in its declaration, and the same holds for the payload fields of one sum variant. A repeat usually means two different fields were intended: rename one, or remove it. A field set twice in a literal is `bynk.resolve.duplicate_field_init`.",
        example: "type Order = { id: Int, id: String }      // ✗ `id` declared twice\n\ntype Order = { id: Int, label: String }  // ✓ distinct names",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.resolve.duplicate_field_init",
        blurb: "A record literal gives each field one value. A field set twice leaves it unclear which value is meant, and is nearly always a pasted line or a misspelt name for the field that was meant. Remove the repeat, or correct the name. A field declared twice in the type is `bynk.resolve.duplicate_field`; a field left out of the literal is `bynk.resolve.missing_field`.",
        example: "type Point = { x: Int, y: Int }\n\nfn origin() -> Point { Point { x: 0, y: 0, y: 1 } }   // ✗ `y` set twice\n\nfn origin() -> Point { Point { x: 0, y: 0 } }         // ✓ each field once",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.resolve.duplicate_message_code",
        blurb: "A message code is the key `render` looks a template up by, so one block can give it only one text. A second entry under the same code leaves two templates for one key, and the second is refused. Delete the repeat, or give it a code of its own. The same code in different locale blocks is how a translation is written, and is not an error. A locale declared twice is `bynk.resolve.duplicate_message_locale`.",
        example: "messages \"en\" @reference {\n    \"cart.empty\" => \"Your basket is empty.\"\n    \"cart.empty\" => \"Your cart is empty.\"       // ✗ already declared in this block\n}\n\nmessages \"en\" @reference {\n    \"cart.empty\" => \"Your basket is empty.\"\n    \"cart.total\" => \"Your total is {total}.\"    // ✓ a code of its own\n}",
        page: "/book/guides/localisation/declare-a-message-bundle",
        anchor: "the-smallest-bundle",
    },
    Explain {
        code: "bynk.resolve.duplicate_message_locale",
        blurb: "A `messages` block holds the templates for one locale, and `render(tag, msg)` picks the block by its tag. Two blocks for one tag leave it two sets of templates to choose from, so the second is refused. Merge them into one block, or correct the tag of the one meant for another language. A code repeated inside a block is `bynk.resolve.duplicate_message_code`.",
        example: "messages \"en\" @reference { … }\nmessages \"en\" { … }                 // ✗ `en` is already declared\n\nmessages \"en\" @reference { … }\nmessages \"fr\" { … }                 // ✓ one block for each locale",
        page: "/book/guides/localisation/declare-a-message-bundle",
        anchor: "adding-a-locale",
    },
    Explain {
        code: "bynk.resolve.duplicate_method",
        blurb: "A method is looked up by its type and name, so `p.norm()` has to mean one body. A second `fn Point.norm` would leave the call with nothing to choose between. Delete the repeat, or give it a different name if it does something else.",
        example: "type Point = { x: Int }\n\nfn Point.norm(self) -> Int { self.x }\n\nfn Point.norm(self) -> Int { 0 }                  // ✗ `Point.norm` is already declared\n\nfn Point.isOrigin(self) -> Bool { self.x == 0 }   // ✓ a different name",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.duplicate_param",
        blurb: "A function's parameters are read by name in its body, and a call passes them in order, so each name may appear once in the list. A repeat is usually two different parameters meant, and the body could see only one of them. Rename one of the two, or remove it.",
        example: "fn area(w: Int, w: Int) -> Int { w * w }   // ✗ `w` declared twice\n\nfn area(w: Int, h: Int) -> Int { w * h }   // ✓ distinct names",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.duplicate_provider",
        blurb: "`provides Clock = …` is the one implementation a context supplies for `Clock`, and a handler that is `given Clock` is given that one. Two providers for the same capability leave it nothing to choose between, so the second is refused. Delete the one you do not want, or merge them into one provider. Declaring the capability itself twice is `bynk.resolve.duplicate_capability`.",
        example: "provides Clock = FixedClock { … }\nprovides Clock = OtherClock { … }          // ✗ `Clock` already has a provider\n\nprovides Clock = FixedClock { … }\nprovides Calendar = FixedCalendar { … }    // ✓ one provider for each capability",
        page: "/book/reference/capabilities",
        anchor: "providing-a-capability",
    },
    Explain {
        code: "bynk.resolve.duplicate_service",
        blurb: "A service is called by its name, and the name is one of the unit's top-level names, so two services of one name leave a call with nothing to choose between. Rename one, or delete the repeat. The same holds for the other named declarations: `bynk.resolve.duplicate_agent`, `bynk.resolve.duplicate_actor`, `bynk.resolve.duplicate_capability` and `bynk.resolve.duplicate_provider`, and for the entries of a message bundle, `bynk.resolve.duplicate_message_code` and `bynk.resolve.duplicate_message_locale`.",
        example: "service orders { … }\nservice orders { … }     // ✗ `orders` is already declared\n\nservice refunds { … }    // ✓ a name of its own",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.duplicate_type",
        blurb: "A unit declares two types, or a type and an event, with the same name. A unit's top-level \
                declarations share one scope in the emitted module, so a name can mean only one thing, \
                and a type name appears in signatures and constructors with no way to say which \
                declaration is meant. Rename one of them, or remove the repeated declaration. The check \
                spans all of the unit's files, so the second declaration can be in a different file.",
        example: "type Money = Int\ntype Money = String          // ✗ `Money` is already declared\n\n\
                  type Money = Int\ntype Label = String          // ✓ distinct names",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.duplicate_variant",
        blurb: "A sum value is identified by its variant name: in a `match` pattern, in a constructor, and as the `kind` of its JSON form. So each variant appears once in a type. A repeat is usually two different variants meant, or a line pasted twice. Rename one of them, or delete the repeat. A payload field repeated within one variant is `bynk.resolve.duplicate_field`.",
        example: "type Colour = | Red | Red     // ✗ `Red` declared twice\n\ntype Colour = | Red | Green   // ✓ distinct variants",
        page: "/book/reference/types",
        anchor: "sum-types",
    },
    Explain {
        code: "bynk.resolve.fn_without_call",
        blurb: "A function's name on its own is the function, not its result. It is a value only where a function type is expected, such as an argument to `map`; anywhere else it is nearly always a forgotten call. Call it with its arguments, or pass it where a function is wanted.",
        example: "fn double(x: Int) -> Int { x * 2 }\n\nfn run() -> Int {\n    let d = double      // ✗ the function itself, not a call\n    5\n}\n\nfn run() -> Int {\n    let d = double(5)   // ✓ the result of calling it\n    d\n}",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.let_shadows_fn",
        blurb: "A `let` may not reuse the name of a function declared in the unit. The name would mean the function in one place and the binding in another, and a reader could not tell which `total` is meant. Choose a different name for the binding. Reusing a type's name is `bynk.resolve.let_shadows_type`.",
        example: "fn total() -> Int { 1 }\n\nfn run() -> Int {\n    let total = 2                // ✗ shadows the function `total`\n    total\n}\n\nfn run() -> Int {\n    let sum = total() + 1        // ✓ a new name\n    sum\n}",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.let_shadows_type",
        blurb: "A `let` may not take the name of a declared type. The same name would then mean a type in one position and a value in another within one body, which is hard to read and easy to get wrong. Choose a different name for the binding, conventionally in lower case. Reusing a function's name is `bynk.resolve.let_shadows_fn`.",
        example: "type Reps = Int where InRange(1, 100)\n\nfn run(n: Int) -> Int {\n    let Reps = n                 // ✗ shadows the type `Reps`\n    n\n}\n\nfn run(n: Int) -> Int {\n    let reps = n                 // ✓ a new name\n    reps\n}",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.method_unknown_type",
        blurb: "A method is declared on a type, as in `fn Order.total(self)`, and the type has to be declared in the same unit, so that every value of it can reach the method. A misspelt or missing type is the usual cause. Correct the name, or declare the type next to the method.",
        example: "fn Order.total(self) -> Int { 1 }         // ✗ there is no type `Order` here\n\ntype Order = { id: Int }\n\nfn Order.total(self) -> Int { self.id }   // ✓ `Order` is declared in this unit",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.missing_field",
        blurb: "A record must be constructed with every field that has no default. \
                There is no partial construction: a value of a record type is only \
                valid once all its fields are present, so a downstream reader never \
                has to wonder whether a field was set. Omitting a required field is \
                therefore an error, not a fill-in-later. A field may be left out \
                only when its declaration gives it a default, `field: T = expr`.",
        example: "type User = { name: String, age: Int }\n\n\
                  User { name: \"Ada\" }            // ✗ missing `age`\n\
                  User { name: \"Ada\", age: 36 }   // ✓ every field present",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.resolve.name_conflict",
        blurb: "Two declarations of different kinds share a name. A unit's functions, assets, embeds, \
                constants, types and events, services, agents and providers with a body (by their \
                implementation name) all live in one namespace, across all of the unit's files, because \
                they are one scope in the emitted module. Capabilities and actors each keep their own. \
                Two types, or a type and an event, are `bynk.resolve.duplicate_type`, and two constants \
                `bynk.resolve.duplicate_const`. Rename one of the two.",
        example: "fn Limit() -> Int {\n    3\n}\n\n\
                  const Limit: Int = 3         // ✗ a function already has this name\n\n\
                  const Limit: Int = 3         // ✓ once the function is renamed `limit`",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.not_a_record_type",
        blurb: "`Name { field: value }` builds a record, so `Name` has to be a record type. A sum is built by naming one of its variants, and a refined type with `Name.of(value)`, which checks the value. A type that is opaque is its own error, `bynk.resolve.opaque_record_construction`.",
        example: "type Shape = | Circle | Square\n\nfn make() -> Shape { Shape { side: 1 } }   // ✗ `Shape` is a sum, not a record\n\nfn make() -> Shape { Circle }              // ✓ name a variant",
        page: "/book/reference/types",
        anchor: "sum-types",
    },
    Explain {
        code: "bynk.resolve.opaque_record_construction",
        blurb: "An opaque type hides its representation so that nothing outside its own checks can produce a value of it, and a record literal `T { value: … }` would go round them. Build one with `T.of(value)`, which validates and returns a `Result`, or, inside the commons that defines the type, with `T.unsafe(value)`.",
        example: "type Timestamp = opaque Int\n\nfn Timestamp.fromMillis(n: Int) -> Timestamp {\n    Timestamp { value: n }       // ✗ record syntax on an opaque type\n}\n\nfn Timestamp.fromMillis(n: Int) -> Timestamp {\n    Timestamp.unsafe(n)          // ✓ inside the defining commons\n}",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.resolve.param_as_function",
        blurb: "Only a value of function type can be called. `n(5)` applies `n`, but `n` is an `Int`, so there is nothing to run. It is usually a missing operator or a parameter that was meant to be a function. Give the parameter a function type, such as `f: Int -> Int`, or drop the call. The check covers any local value, not only parameters.",
        example: "fn apply(n: Int) -> Int {\n    n(5)                         // ✗ `n` is an `Int`, not a function\n}\n\nfn apply(f: Int -> Int) -> Int {\n    f(5)                         // ✓ a function-typed parameter\n}",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.recursive_record_field",
        blurb: "A record cannot contain itself through record fields alone, whether directly (`next: Node` in `Node`) or round a cycle (`A` has a `B`, which has an `A`). A value of such a type could never be built, since making one needs one already, and the compiler could not synthesise the default value it needs for one. A cycle through an `Option`, `List`, `Map` or a sum type is fine, because that position can hold a value that ends the chain. Wrap one field in the cycle in `Option[…]`.",
        example: "type Node = { value: Int, next: Node }   // ✗ a `Node` would need a `Node`\n\ntype Node = { value: Int, next: Option[Node] }   // ✓ `None` ends the chain",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.resolve.reserved_builtin_type",
        blurb: "The compiler knows a few type names itself, and the type parser reads each one as the built-in wherever it appears in a type. A `type` declaration that reused one would be shadowed at every use, or fail confusingly there, so the declaration is refused. The names are `Asset`, `BoundaryRejection`, `Connection`, `History`, `HttpResult`, `List`, `Map`, `Query`, `QueueResult`, `RequestHeaders`, `RequestQuery`, `Set`, `Stream` and `TokenClaims`. Rename the type. They are reserved in type position only, so a value may still use the word. A constant with such a name is `bynk.resolve.name_conflict` instead, and the base types and words such as `Result` are keywords that the parser refuses earlier.",
        example: "type Set = { items: List[Int] }   // ✗ `Set` is the built-in set type\n\ntype IntSet = { items: List[Int] }   // ✓",
        page: "/book/reference/keywords",
        anchor: "built-in-type-names",
    },
    Explain {
        code: "bynk.resolve.reserved_host_name",
        blurb: "The generated TypeScript reaches the host's built-in objects as `globalThis.<name>`, so a top-level declaration called `globalThis` (a type, function, agent, provider and so on) would hide all of them in its module. Rename the declaration. Parameters and local bindings may use the name: the compiler renames those itself.",
        example: "type globalThis = { n: Int }   // ✗ the name is reserved\n\ntype Counter = { n: Int }      // ✓ any other name",
        page: "/book/spec/emission",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.reserved_payload_field",
        blurb: "A sum value is encoded as a flat JSON object whose `kind` field holds the variant's name, so a payload field called `kind` would collide with that tag. Rename the field, for instance to `category`.",
        example: "type Shape = | Circle(kind: String, r: Int) | Square(side: Int)   // ✗ `kind` is the variant tag\n\ntype Shape = | Circle(category: String, r: Int) | Square(side: Int)   // ✓ any other name",
        page: "/book/reference/types",
        anchor: "sum-types",
    },
    Explain {
        code: "bynk.resolve.self_outside_method",
        blurb: "`self` is the value a method was called on, so it exists only in the body of a method, declared as `fn Type.name(self, …)`. A plain function has no receiver. Make the function a method of the type, or take the value as an ordinary parameter.",
        example: "type Counter = { n: Int }\n\nfn read(c: Counter) -> Int {\n    self.n                         // ✗ a plain function has no `self`\n}\n\nfn Counter.read(self) -> Int {     // ✓ a method declares `self`\n    self.n\n}",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.type_as_function",
        blurb: "A type's name is not a function. A refined type is built with `.of(value)`, which checks the value and returns a `Result`, and a record with a literal such as `Order { … }`, so `Amount(5)` has no meaning to fall back on. A sum variant with a payload is the one exception: `Placed(at, total)` constructs it. A name that is not a type at all is `bynk.resolve.unknown_function`.",
        example: "type Amount = Int where NonNegative\n\nfn price() -> Amount {\n    Amount(5)                    // ✗ a type is not a function\n}\n\nfn price() -> Result[Amount, ValidationError] {\n    Amount.of(5)                 // ✓ checked construction\n}",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.resolve.type_in_expr",
        blurb: "A type's name stands for a set of values, not for one, so it cannot be used where a value is expected. This is usually a missing constructor. Build a value in the type's own way: `Type { … }` for a record, a variant name for a sum, `Type.of(value)` for a refined type. A type used as a call, `Type(…)`, is `bynk.resolve.type_as_function`.",
        example: "type Metres = Int where NonNegative\n\nfn grow(x: Int) -> Int { x + Metres }   // ✗ `Metres` is a type, not a value\n\nfn grow(x: Int) -> Result[Metres, ValidationError] { Metres.of(x + 1) }   // ✓ build a `Metres`",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.unconsumed_context",
        blurb: "A call such as `platform.pay.charge(1)` reaches into another context, and a context may only do that to one it has declared with `consumes`. The declaration is what makes a dependency between contexts visible and checked, instead of a name that happens to resolve. The same code is reported for a `given platform.pay.Cap` whose context is not consumed. Add `consumes platform.pay` (with an alias, if you want to call it by another name) at the top of the calling context. A consumed context that does not export a capability is `bynk.given.cross_context_unknown_capability`.",
        example: "// app/shop.bynk\ncontext app.shop\n\nservice checkout {\n    on call() -> Effect[Int] {\n        platform.pay.charge(1)   // ✗ `platform.pay` is not in this context's `consumes`\n    }\n}\n\n\
                  context app.shop\n\nconsumes platform.pay          // ✓ declared at the top",
        page: "/book/guides/program-structure/consume-services",
        anchor: "declare-the-dependency-and-call-the-service",
    },
    Explain {
        code: "bynk.resolve.unknown_field",
        blurb: "A record construction names a field the record type does not declare. \
                A record's fields are fixed by its type declaration, and a literal may \
                set only those. This is usually a typo in the field name, or a field \
                meant for a different type. Reading a field a value does not have is \
                the sibling code `bynk.types.unknown_field`.",
        example: "type User = { name: String, age: Int }\n\n\
                  User { name: \"Ada\", age: 36, admin: true }   // ✗ no field `admin`\n\
                  User { name: \"Ada\", age: 36 }                // ✓ only the declared fields",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.resolve.unknown_function",
        blurb: "A call names a function that is not in scope. Only functions declared in the same unit, or \
                brought in through `uses`, can be called; a capitalised name, which would construct an \
                agent, needs an agent of that name in scope. This is most often a typo or a missing \
                `uses`. A lambda cannot call itself through its own `let` name either, because the name \
                is not bound until the lambda is built. A type used as a function, as in `Money(5)`, is \
                the separate `bynk.resolve.type_as_function`.",
        example: "fn double(x: Int) -> Int {\n    x * 2\n}\n\n\
                  dubble(4)          // ✗ no function `dubble`\n\n\
                  double(4)          // ✓ the declared name",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.unknown_name",
        blurb: "A name was referenced that is not in scope. Every name in Bynk \
                must be introduced before use — as a `let` binding, a parameter, \
                a `fn`, a type, or a member brought in through `uses`/`consumes`. \
                An unknown name is typically a typo, a missing declaration, or a \
                reference to something defined in a module that has not been \
                brought into scope.",
        example: "let greeting = \"hi\"\n\
                  greetng          // ✗ no name `greetng` in scope\n\
                  greeting         // ✓ the bound name",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "",
    },
    Explain {
        code: "bynk.resolve.unknown_static_member",
        blurb: "A type was named with a `.` and a member it does not have. `T.name` refers to a variant of the \
                sum type `T` or to a static method declared on it, and a misspelt variant has no such member. Some built-in types \
                have a closed set: `HttpResult`, `ErrorStatus` and `RoundingMode` only their variants, \
                `Float` only `parse`, `pi` and `e`, and `Decimal` only `parse` and `fromInt`. Check the spelling against the declaration, or declare the missing \
                static method.",
        example: "type Colour =\n    | Red\n    | Green\n\n\
                  Colour.Gren          // ✗ `Colour` has no variant `Gren`\n\n\
                  Colour.Green         // ✓ the declared variant",
        page: "/book/reference/types",
        anchor: "sum-types",
    },
    Explain {
        code: "bynk.resolve.unknown_type",
        blurb: "A type name was referenced that does not exist. Types must be \
                declared (with `type`), be one of Bynk's built-in types, or be \
                brought into scope from another module before they can be named. \
                An unknown type is usually a typo or a missing declaration/import.",
        example: "fn greet(u: Usr) -> String { … }   // ✗ no type `Usr`\n\
                  fn greet(u: User) -> String { … }  // ✓ the declared type",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.send.deprecated",
        blurb: "`~>` is deprecated: `e.detach()` says the same thing as an ordinary effect statement, works for any `Effect[T]` and inside a lambda, `forEach` or helper, and reports a fault or an `Err` instead of losing it. A warning, not an error: `bynk check --fix` rewrites each `~> e` to `do e.detach()`. `~>` will be removed in the next release, after which it is a parse error. For a unit effect that does not need to outlive the handler, plain `do e` is enough; `Logger` calls finish before they return.",
        example: "on call bump() -> Effect[()] given Logger {\n    ~> Logger.info(\"bump\")             // ⚠ deprecated\n    Effect.pure(())\n}\n\n\
                  on call bump() -> Effect[()] given Logger {\n    do Logger.info(\"bump\").detach()   // ✓ the same, as an ordinary statement\n    Effect.pure(())\n}",
        page: "/book/reference/capabilities",
        anchor: "detach",
    },
    Explain {
        code: "bynk.send.in_agent_handler",
        blurb: "`~>` starts an effect and lets the handler carry on, so something must keep the effect alive after the handler returns. A service handler has an execution context that does. An agent handler runs inside the agent's own object, with no such context, and its effects are awaited so that they finish with the handler. So the send is refused, and so is one inside a closure there. Await it instead, with `do` for an `Effect[()]` or `let _ <-`, or move the send to the service handler that calls the agent. A `~>` of a store operation is `bynk.send.store_op`.",
        example: "on call bump() -> Effect[()] given Logger {\n    ~> Logger.info(\"bump\")      // ✗ an agent handler cannot send\n    do count.update((n) => n + 1)\n}\n\n\
                  on call bump() -> Effect[()] given Logger {\n    do Logger.info(\"bump\")      // ✓ awaited, so it finishes with the handler\n    do count.update((n) => n + 1)\n}",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.send.in_pure_context",
        blurb: "A `~>` send starts an effect and does not wait for it, so it can only appear in a body that runs effects: one that returns `Effect[…]`. A pure function never runs one, so there would be nothing to send. Change the enclosing function's return type to `Effect[…]`. The same rule holds for `bynk.within.in_pure_context` and `bynk.race.in_pure_context`.",
        example: "fn twice(ping: Effect[()], n: Int) -> Int {\n    ~> ping   // ✗ the body is pure\n    n * 2\n}\n\nfn twice(ping: Effect[()], n: Int) -> Effect[Int] {   // ✓ an effectful body\n    ~> ping\n    Effect.pure(n * 2)\n}",
        page: "/book/reference/capabilities",
        anchor: "capabilities-in-functions",
    },
    Explain {
        code: "bynk.send.non_effect",
        blurb: "`~>` sends an effectful call without waiting for it, so what follows it must be an `Effect`. A plain value such as an `Int` has nothing to run and nothing to send. If the call is meant to be a plain call, drop the `~>`; if it is meant to be sent, make the function return `Effect[()]`. A send of an effect with a reply is `bynk.send.requires_unit`.",
        example: "fn total() -> Int { 1 }\n\nfn run() -> Effect[()] {\n    ~> total()   // ✗ `total` returns an `Int`, not an `Effect`\n    Effect.pure(())\n}\n\nfn total() -> Effect[()] { Effect.pure(()) }\n\nfn run() -> Effect[()] {\n    ~> total()   // ✓ an `Effect[()]`\n    Effect.pure(())\n}",
        page: "/book/guides/effects-and-capabilities/synchronous-and-asynchronous-sends",
        anchor: "the-error-gate",
    },
    Explain {
        code: "bynk.send.requires_unit",
        blurb: "`~>` never waits, so it throws away whatever the call returns. Bynk therefore lets it send only an operation whose reply is `Effect[()]`: an `Effect[Int]`, or one carrying a `Result`, would lose its value or its error with no trace. To use the reply, or to discard it on purpose, await the call with `let r <- …` or `let _ <- …`. For a value that is not an `Effect` at all see `bynk.send.non_effect`.",
        example: "fn fetch() -> Effect[Int] { Effect.pure(1) }\n\nfn run() -> Effect[()] {\n    ~> fetch()   // ✗ the `Int` reply would vanish\n    Effect.pure(())\n}\n\nfn run() -> Effect[()] {\n    let _ <- fetch()   // ✓ awaited, the reply discarded on purpose\n    Effect.pure(())\n}",
        page: "/book/guides/effects-and-capabilities/synchronous-and-asynchronous-sends",
        anchor: "the-error-gate",
    },
    Explain {
        code: "bynk.send.store_op",
        blurb: "In an agent handler, a store write is not a message to send: it is staged and committed atomically with the rest of the handler, so there is nothing for `~>` to leave running. `~>` is refused there for any call, and a store operation gets its own code to point at the fix. Drop the `~>` and write `do field.op(…)`, or `let _ <- field.op(…)`; the write is committed with the handler. For a send that is not a store operation see `bynk.send.in_agent_handler`.",
        example: "on call bump() -> Effect[()] {\n    ~> count.update((n) => n + 1)   // ✗ a store write is not sent\n}\n\non call bump() -> Effect[()] {\n    do count.update((n) => n + 1)   // ✓ staged, committed with the handler\n}",
        page: "/book/guides/effects-and-capabilities/synchronous-and-asynchronous-sends",
        anchor: "not-in-an-agent-handler",
    },
    Explain {
        code: "bynk.service.missing_from",
        blurb: "A service with no `from` clause is a call service, and admits only `on call` handlers; the protocol in the header is what says how a handler is invoked. A handler of another form needs its protocol named: `from http` for a route, `from cron`, `from queue(\"…\")`, `from websocket(…)` or `from Events(E)`. The error suggests the one that fits the handler. If the service really is a mix, split it (see `bynk.service.mixed_protocols`).",
        example: "service worker {\n    on message(m: String) -> Effect[QueueResult] { … }   // ✗ no `from` clause\n}\n\nservice worker from queue(\"jobs\") {   // ✓ the protocol is named\n    on message(m: String) -> Effect[QueueResult] { … }\n}",
        page: "/book/reference/grammar",
        anchor: "rule-service_protocol",
    },
    Explain {
        code: "bynk.service.mixed_protocols",
        blurb: "A service is one protocol adapter: with `from http`, `from cron`, `from queue(…)`, `from websocket(…)` or `from Events(E)` it admits only handlers of that form, and one that does not match is refused. Declare a service per protocol. A handler in a service with no `from` at all is `bynk.service.missing_from`.",
        example: "service worker from queue(\"jobs\") {\n    on call(m: String) -> Effect[Int] { … }   // ✗ `on call` is not a queue handler\n}\n\nservice count {                             // ✓ a service of its own\n    on call(m: String) -> Effect[Int] { … }\n}",
        page: "/book/reference/grammar",
        anchor: "rule-service_protocol",
    },
    Explain {
        code: "bynk.service.outside_context",
        blurb: "A `service` is a context's entry point: a call, a route, a schedule, a queue consumer, a socket. A `commons` holds only shared types and pure functions, and has no context to serve from. Move the service into a context. Logic it shares can stay in the commons, which the context then `uses`.",
        example: "commons shop\n\nservice probe { … }   // ✗ a commons holds no entry points\n\ncontext shop\n\nservice probe { … }   // ✓",
        page: "/book/reference/grammar",
        anchor: "rule-service_decl",
    },
    Explain {
        code: "bynk.service.return_not_effect",
        blurb: "Every service handler returns an `Effect[T]`, whatever its protocol. A handler runs as the context answers a call, route, schedule, message or frame, and its effects and `given` capabilities are the runtime's to run, so a bare `Int` is refused. Wrap the result: `Effect.pure(x)` for a value you already hold. An `on event` handler that returns an `Effect` of anything but `()` is `bynk.event.return_not_effect_unit`.",
        example: "service probe {\n    on call() -> Int { … }            // ✗ not an `Effect`\n    on call() -> Effect[Int] { … }    // ✓\n}",
        page: "/book/reference/grammar",
        anchor: "rule-service_decl",
    },
    Explain {
        code: "bynk.service.unknown_protocol",
        blurb: "The protocol after `from` is one of a closed set: `http`, `cron`, `queue`, `websocket` or `Events`. Kafka and MQTT are transports, not protocols: use `from queue(\"…\")` and bind the broker at the platform layer. A service with no `from` takes `on call` handlers.",
        example: "service worker from kafka {                // ✗ not a protocol\n\nservice worker from queue(\"jobs\") {        // ✓ the queue protocol",
        page: "/book/reference/grammar",
        anchor: "rule-service_protocol",
    },
    Explain {
        code: "bynk.service.unknown_via_clause",
        blurb: "A `via` clause narrows which events an `Events` subscriber receives, and the only form is `via schema(N)`, which selects by the event's schema version. The set of `via` clauses is closed, so any other word after `via` is refused. Use `via schema(N)` or drop the clause. A malformed argument to `schema` itself is `bynk.event.bad_schema_dispatch`.",
        example: "service OnPaid from Events(Paid) via nope(1) {   // ✗ only `schema` follows `via`\n    on event(e: Paid) -> Effect[()] { … }\n}\n\nservice OnPaid from Events(Paid) via schema(1) {   // ✓ dispatch on the schema version\n    on event(e: Paid) -> Effect[()] { … }\n}",
        page: "/book/guides/events/understand-events",
        anchor: "dispatching-by-version-with-via-scheman",
    },
    Explain {
        code: "bynk.service.websocket_header",
        blurb: "A WebSocket service names its two frame types in the header, `from websocket(in: ClientFrame, out: ServerFrame)`: `in` is what the server decodes from the client and `out` is what it sends back. The labels are fixed and in that order, so the compiler can tell the two apart without guessing from the handlers. Spell them `in:` then `out:`.",
        example: "service gateway from websocket(input: Frame, out: Frame) {   // ✗ the first label must be `in`\n    on open (room: RoomId) -> Effect[()] by user: Member { … }\n}\n\nservice gateway from websocket(in: Frame, out: Frame) {   // ✓\n    on open (room: RoomId) -> Effect[()] by user: Member { … }\n}",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
    Explain {
        code: "bynk.service.websocket_multiple",
        blurb: "A WebSocket upgrade reaches the context by its `Upgrade: websocket` header alone, with no path to tell two endpoints apart, so a context can host only one `from websocket` service. The compiler reports every one but the first by name. Keep one, or move the other into a context of its own.",
        example: "service lobby from websocket(in: Frame, out: Frame) { … }\nservice rooms from websocket(in: Frame, out: Frame) { … }   // ✗ a second WebSocket service in this context\n\nservice lobby from websocket(in: Frame, out: Frame) { … }   // ✓ one per context",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
    Explain {
        code: "bynk.service.websocket_open_arity",
        blurb: "A WebSocket service has exactly one `on open`, the handler that accepts the upgrade and receives the connection, and at most one `on message` and one `on close`. A service with no `on open` could never accept a client, and a second of any handler would leave it unclear which one runs. Add the missing `on open`, or remove the duplicate.",
        example: "service gateway from websocket(in: Frame, out: Frame) {   // ✗ no `on open`\n    on message (room: RoomId, frame: Frame) -> Effect[()] by user: Member { … }\n}\n\nservice gateway from websocket(in: Frame, out: Frame) {\n    on open (room: RoomId) -> Effect[()] by user: Member { … }   // ✓\n    on message (room: RoomId, frame: Frame) -> Effect[()] by user: Member { … }\n}",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
    Explain {
        code: "bynk.service.websocket_param_not_stringy",
        blurb: "An `on open` parameter is read from the upgrade request's query string, so it arrives as text, and the compiler builds it with the type's constructor from a `String`. A type that cannot be built from a string, such as `Int`, has no safe way in: a stray `?room=05` would name a different value from `5`. Declare the parameter as a `String`, a refined `String` or an opaque type over `String`, and parse any number in the body. HTTP path parameters follow the same rule, as `bynk.http.path_param_not_stringy`.",
        example: "service gateway from websocket(in: Frame, out: Frame) {\n    on open (room: RoomId, limit: Int) -> Effect[()] by user: Member { … }   // ✗ a query value is a string\n}\n\nservice gateway from websocket(in: Frame, out: Frame) {\n    on open (room: RoomId, limit: String) -> Effect[()] by user: Member { … }   // ✓ parse the number in the body\n}",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
    Explain {
        code: "bynk.stability.experimental",
        blurb: "Some standard-library items are experimental: they may change or disappear in any release, and the compatibility promise does not cover them. Using one is an error until the project opts in, so that relying on it is a choice someone made and can find. Add the item's path, as the error gives it, to `allow` under `[unstable]` in `bynk.toml`, or avoid the item. An entry that names no standard-library item is `bynk.project.unstable_unknown_item`.",
        example: "// demo.bynk\nconsumes bynk.cloudflare { Version }    // ✗ experimental\n\n// bynk.toml\n[unstable]\nallow = [\"bynk.cloudflare.Version\"]     // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "stability",
    },
    Explain {
        code: "bynk.store.annotation_kind_mismatch",
        blurb: "Each storage annotation is for one kind of store: `@ttl` for a `Cache`, `@retain` for a `Log` and `@indexed` for a `Map`. On any other kind it has nothing to act on: a `Map` has no expiry, for one. Move the annotation to a field of the kind it belongs to, or change the field's kind if that is what you meant.",
        example: "store sessions: Map[String, Int] @ttl(30.minutes)     // ✗ `@ttl` is for a `Cache`\n\nstore sessions: Cache[String, Int] @ttl(30.minutes)   // ✓",
        page: "/book/reference/agents",
        anchor: "storage-annotations",
    },
    Explain {
        code: "bynk.store.annotation_unsupported",
        blurb: "A storage annotation has a fixed set of kinds it applies to and a meaning that arrives in stages. `@bounded` is recognised on `Queue` and `Log`, but nothing acts on it yet, so it is refused rather than accepted and ignored. Leave it off. To limit a `Log`, use `@retain`, which drops entries older than a period. A known annotation on the wrong kind is `bynk.store.annotation_kind_mismatch`.",
        example: "store trail: Log[Event] @bounded(100)     // ✗ `@bounded` is recognised but has no effect yet\n\nstore trail: Log[Event] @retain(30.days)   // ✓ keep 30 days of entries",
        page: "/book/reference/agents",
        anchor: "storage-annotations",
    },
    Explain {
        code: "bynk.store.cache_needs_clock",
        blurb: "A `Cache` entry expires, and every operation on it but `remove` reads the clock to decide whether an entry is still live. So a handler that uses a `Cache` must declare `given Clock`, the same way one that reads the time itself would; the dependency is explicit even though the body never names the clock. Add `given Clock` to the handler's signature.",
        example: "consumes bynk { Clock }\n\nstore live: Cache[String, Int] @ttl(5.minutes)\n\non call touch() -> Effect[()] {\n    let _ <- live.put(\"a\", 1)   // ✗ expiry reads the clock, which the handler does not declare\n    Effect.pure(())\n}\n\non call touch() -> Effect[()] given Clock {   // ✓ declare the clock\n    let _ <- live.put(\"a\", 1)\n    Effect.pure(())\n}",
        page: "/book/reference/agents",
        anchor: "storage-kinds-and-their-operations",
    },
    Explain {
        code: "bynk.store.cache_ttl_required",
        blurb: "A `Cache` is a keyed store whose entries expire, so it needs `@ttl(<duration>)`, how long an entry lives. Without an expiry it would only be a `Map`. The argument must be a `Duration` literal such as `5.minutes`; a bare number (`@ttl(5)`) or a computed value is refused too, because the lifetime has to be known when the program compiles. Add the annotation, or declare a `Map` if entries should be kept until removed.",
        example: "store live: Cache[String, Int]   // ✗ no lifetime for the entries\n\nstore live: Cache[String, Int] @ttl(5.minutes)   // ✓\n\nconst Life: Duration = 5.minutes\nstore quick: Cache[String, Int] @ttl(Life)   // ✓ a `Duration` constant bound to a literal",
        page: "/book/reference/agents",
        anchor: "storage-annotations",
    },
    Explain {
        code: "bynk.store.kind_arity",
        blurb: "Each storage kind takes a fixed number of type arguments: `Cell[T]`, `Set[T]` and `Log[T]` take one, and `Map[K, V]` and `Cache[K, V]` take two. The checker reads the arguments by position, so a wrong count cannot be guessed at and is refused where the field is declared. Give the kind the arguments it takes. To keep a key with a value use `Map`, and to hold a single value use `Cell[T]`.",
        example: "store total: Cell[String, Int]   // ✗ a `Cell` takes one type argument\n\nstore total: Cell[Int]   // ✓ one",
        page: "/book/reference/agents",
        anchor: "storage-kinds-and-their-operations",
    },
    Explain {
        code: "bynk.store.kind_unsupported",
        blurb: "`Queue` is a recognised storage kind but is not yet implemented. The working kinds are `Cell`, `Map`, `Set`, `Log` and `Cache`, so a `store` of any other known kind is refused. Use one of them: an append-only `Log` is the closest to a queue. A name that is no storage kind at all is `bynk.store.unknown_kind`.",
        example: "store pending: Queue[Event]                 // ✗ not yet supported\n\nstore pending: Log[Event] @retain(30.days)   // ✓ an append-only sequence",
        page: "/book/reference/agents",
        anchor: "storage-kinds-and-their-operations",
    },
    Explain {
        code: "bynk.store.log_needs_clock",
        blurb: "`Log.append` stamps each entry with the time it was written, so it reads the clock and the handler has to declare `given Clock`. The window reads (`since`, `before`, `between`) take explicit `Instant`s and need no clock. Add `given Clock` to the handler that appends.",
        example: "consumes bynk { Clock }\n\nstore trail: Log[String]\n\non call opened() -> Effect[()] {\n    let _ <- trail.append(\"opened\")   // ✗ the stamp reads the clock\n    Effect.pure(())\n}\n\non call opened() -> Effect[()] given Clock {   // ✓ declare the clock\n    let _ <- trail.append(\"opened\")\n    Effect.pure(())\n}",
        page: "/book/reference/agents",
        anchor: "storage-kinds-and-their-operations",
    },
    Explain {
        code: "bynk.store.unkeyable_key",
        blurb: "A store `Map` or `Cache` key, or a store `Set` element, must be `String`, `Int` or a refined or opaque type over them. A store collection is saved as a JSON object keyed by the key's string form, and a record has no stable string form, so it cannot be a key (a composite key is not supported yet). Key by one of the record's id fields, or by an enum's name as a `String`. A value `Map` or `Set` follows a wider rule, `bynk.types.not_keyable`.",
        example: "type Point = { x: Int, y: Int }\n\nstore cells: Map[Point, Int]   // ✗ a record has no stable string key\n\nstore cells: Map[String, Int]   // ✓ key by a string, such as an id",
        page: "/book/reference/agents",
        anchor: "storage-kinds-and-their-operations",
    },
    Explain {
        code: "bynk.store.unknown_annotation",
        blurb: "A `store` field takes one of four annotations: `@indexed`, `@ttl`, `@retain` and `@bounded`. The set is closed, so a name outside it, usually a misspelling or one borrowed from another system, is refused rather than ignored. Use the annotation that says what you mean: `@retain` for how long a `Log` keeps entries, `@ttl` for a `Cache`. A known one on the wrong kind is `bynk.store.annotation_kind_mismatch`.",
        example: "store audit: Log[String] @keep(7.days)     // ✗ not a storage annotation\n\nstore audit: Log[String] @retain(7.days)   // ✓ how long a `Log` keeps entries",
        page: "/book/reference/agents",
        anchor: "storage-annotations",
    },
    Explain {
        code: "bynk.store.unknown_kind",
        blurb: "The type of a `store` field names a storage kind, which says how the state is persisted and which operations it has; it is not an ordinary type. The kinds are `Cell`, `Map`, `Set`, `Log` and `Cache`; `Queue` is reserved but not yet supported (`bynk.store.kind_unsupported`). A `List` or a record is not one, because nothing would say how to read or update it atomically. Pick the kind that matches how you use the data: `Set` for membership, `Map` for lookup by key, `Log` to append, `Cell` for one value.",
        example: "store tags: List[String]   // ✗ `List` is not a storage kind\n\nstore tags: Set[String]   // ✓ a storage kind",
        page: "/book/reference/agents",
        anchor: "storage-kinds-and-their-operations",
    },
    Explain {
        code: "bynk.store.unknown_map_accessor",
        blurb: "A `store Map` field is read as a lazy query through exactly three accessors: `.entries` (each a `MapEntry` with `.key` and `.value`), `.keys` and `.values`. Any other name after the field is refused. Its effectful operations, such as `put` and `get`, are method calls, not accessors. Use one of the three, or call an operation. A wrong method name on a store is `bynk.store.unknown_op`.",
        example: "store items: Map[String, Item]\n\nitems.names.collect()   // ✗ a `Map` has no `.names`\n\nitems.keys.collect()   // ✓ the keys, as a query",
        page: "/book/reference/types",
        anchor: "map-key-accessors",
    },
    Explain {
        code: "bynk.store.unknown_op",
        blurb: "A `store` field's methods are its storage operations, which are effectful (await with `<-`), and each kind has a fixed set. `Map` and `Cache` take `put`, `get`, `update`, `upsert`, `remove`, `contains` and `size` (a `Map` also takes query operations); `Set` takes `add`, `remove`, `contains` and `size`; a `Cell` takes only `update`, being read by name and written with `:=`; a `Log` takes `append` and lazy query reads. Any other name is refused, even one the same type has as a plain value. For a read-modify-write use `update`, or `upsert` when the key may be absent. A wrong key query (`.entries`, `.keys`, `.values`) is `bynk.store.unknown_map_accessor`.",
        example: "let _ <- counts.increment(\"k\")   // ✗ a `Map` store has no `increment`\n\nlet _ <- counts.upsert(\"k\", 1, (v) => v + 1)   // ✓ add one, starting from 1",
        page: "/book/reference/agents",
        anchor: "storage-kinds-and-their-operations",
    },
    Explain {
        code: "bynk.stub.after_type",
        blurb: "`stub … returns <value> after <delay>` makes the stubbed call \
                complete only once the harness clock has advanced by that \
                delay, so the delay must be a `Duration`, such as `3.seconds`. \
                A bare number has no unit, so it is refused rather than \
                guessed at as milliseconds or seconds.",
        example: "// demo/meter.bynk\n\
                  capability Meter { … }\n\n\
                  // tests/meter.test.bynk\n\
                  stub Meter.read() returns 7 after 3           // ✗ no unit\n\
                  stub Meter.read() returns 7 after 3.seconds   // ✓",
        page: "/book/reference/testing",
        anchor: "stub-latency",
    },
    Explain {
        code: "bynk.stub.at_system",
        blurb: "At the `system` tier each context runs as the Worker it deploys as, and that Worker builds its own capability providers, so there is no seam for a `stub` to override. A `stub` there would be accepted and then ignored, so it is refused. Move the case to a suite below `system`, where a `stub` replaces the seam, or remove the stub and test the real provider.",
        example: "suite demo.desk as system {\n    stub Meter.read() returns 7   // ✗ a `system` suite has no seam to stub\n\n    case \"the desk checks\" { … }\n}\n\nsuite demo.desk {\n    stub Meter.read() returns 7   // ✓ below `system` the seam can be stubbed\n\n    case \"the desk checks\" { … }\n}",
        page: "/book/reference/testing",
        anchor: "stub",
    },
    Explain {
        code: "bynk.stub.bad_sequence",
        blurb: "`returns each […]` gives one outcome per call, in order, and the \
                last outcome repeats once the list runs out. An empty list has no \
                outcome to repeat, so the first call would have nothing to return. \
                List at least one. To return an empty list as a single value, \
                write `returns []`, without `each`.",
        example: "// demo/meter.bynk\n\
                  capability Meter { … }\n\
                  \n\
                  // tests/meter.test.bynk\n\
                  stub Meter.read() returns each []    // ✗ no outcomes\n\
                  stub Meter.read() returns each [7]   // ✓ at least one",
        page: "/book/reference/testing",
        anchor: "stub",
    },
    Explain {
        code: "bynk.stub.generic_op",
        blurb: "A `stub` gives an operation a fixed answer, but an operation with its own type parameter, `fn dedup[T](…)`, answers in whatever type the caller names, and a fixed value cannot be that. Such an operation cannot be stubbed. Test through the capability's real provider, and stub the other, non-generic operations of the capability if you need to.",
        example: "// tests/orders.test.bynk\nstub Idempotency.dedup(_) returns None   // ✗ `dedup` has its own type parameter\n\ncase \"reserve returns the cached value\" {\n    let result <- reserve.call()   // ✓ the real provider answers\n    expect result is None\n}",
        page: "/book/reference/capabilities",
        anchor: "generic-operations",
    },
    Explain {
        code: "bynk.stub.not_a_seam",
        blurb: "A `stub` substitutes one of the capabilities the unit under test \
                depends on: one the context declares, or reaches through a \
                `consumes`. Any other name, including a misspelt one, leaves \
                nothing to substitute. A commons has no capabilities, so it cannot \
                be stubbed at all. Name a capability of the target, or add the \
                `consumes` it needs. See `bynk.stub.unknown_op` for a wrong \
                operation.",
        example: "// demo/meter.bynk\n\
                  capability Meter { … }\n\
                  \n\
                  // tests/meter.test.bynk\n\
                  stub Gauge.read() returns 7   // ✗ the context has no `Gauge`\n\
                  stub Meter.read() returns 7   // ✓",
        page: "/book/reference/testing",
        anchor: "stub",
    },
    Explain {
        code: "bynk.stub.rhs_type",
        blurb: "A `stub … returns <value>` stands in for a real provider, so the \
                value must have the type the operation declares, as the real \
                result would. `Meter.read` yields an `Int`, so it returns `7`, not \
                a string; the `Effect` is supplied for you. Change the value, or \
                stub the operation you meant. To inject a failure, write `fails` \
                instead of a value.",
        example: "// demo/meter.bynk\n\
                  capability Meter { … }\n\
                  \n\
                  // tests/meter.test.bynk\n\
                  stub Meter.read() returns \"seven\"   // ✗ a String, not an Int\n\
                  stub Meter.read() returns 7         // ✓",
        page: "/book/reference/testing",
        anchor: "stub",
    },
    Explain {
        code: "bynk.stub.unknown_op",
        blurb: "`stub Cap.op(…)` overrides one operation of a capability, so `op` \
                must be declared by that capability. A misspelt or renamed \
                operation matches nothing, so the stub is refused instead of \
                leaving the default provider in place unannounced. Use the \
                operation name the capability declares. See `bynk.stub.not_a_seam` \
                when the capability itself is the wrong one.",
        example: "// demo/meter.bynk\n\
                  capability Meter { … }\n\
                  \n\
                  // tests/meter.test.bynk\n\
                  stub Meter.reed() returns 7   // ✗ Meter has no `reed`\n\
                  stub Meter.read() returns 7   // ✓",
        page: "/book/reference/testing",
        anchor: "stub",
    },
    Explain {
        code: "bynk.suite.const_name_clash",
        blurb: "A suite's cases see the target's own declarations and those of the commons it `uses`, and the suite's constants are used by bare name beside them. A suite constant with one of those names, or with that of a sum variant in scope, would silently change which value a mention reaches, so it is refused instead of shadowing. Rename it. A type clashing is `bynk.suite.type_name_clash`.",
        example: "// demo/desk.bynk\nconst Limit: Int = 5\n\n// tests/desk.test.bynk\nconst Limit: Int = 9    // ✗ the target already has a `Limit`\nconst Cap: Int = 9      // ✓ a name of its own",
        page: "/book/reference/testing",
        anchor: "suite-types-consts",
    },
    Explain {
        code: "bynk.suite.duplicate_case_name",
        blurb: "A case is identified by its description in the runner's output and \
                in failure reports, so each description must be unique across the \
                suites that test one unit. Two cases with the same description \
                cannot be told apart. Rename one; the error is on the repeat, with a label at the first.",
        example: "// tests/ledger.test.bynk\n\
                  case \"an entry\" { … }\n\
                  case \"an entry\" { … }          // ✗ same description\n\
                  case \"another entry\" { … }     // ✓",
        page: "/book/reference/testing",
        anchor: "suite-blocks",
    },
    Explain {
        code: "bynk.suite.duplicate_const",
        blurb: "A suite's constants are used by bare name in its cases, so a suite declares each name once: a second `const Limit` would leave `Limit` meaning two values. A `const` named like one of the suite's `type`s counts as a repeat too. Rename one. The same holds for a suite's types, as `bynk.suite.duplicate_type`, and for helper functions, as `bynk.suite.duplicate_fn`.",
        example: "const Limit: Int = 5\nconst Limit: Int = 9    // ✗ `Limit` again\nconst Cap: Int = 9      // ✓ a name each",
        page: "/book/reference/testing",
        anchor: "suite-types-consts",
    },
    Explain {
        code: "bynk.suite.duplicate_env",
        blurb: "An `env` clause gives a variable one value in its scope, so naming it twice in the same suite or the same case leaves two values and no way to choose. Keep one. A case's own `env` may still override a name its suite gives; only a repeat within one scope is refused.",
        example: "env {\n    MODE = \"fake\"\n    MODE = \"live\"     // ✗ `MODE` is already given in this suite\n}\n\nenv {\n    MODE = \"fake\"\n    REGION = \"eu\"     // ✓ one value per name\n}",
        page: "/book/reference/testing",
        anchor: "env",
    },
    Explain {
        code: "bynk.suite.duplicate_fn",
        blurb: "A case calls a suite helper by its bare name, so two helpers \
                of one suite with one name would leave the call ambiguous. \
                Give each a distinct name; the error is on the repeat. The \
                same holds for a suite's types and constants: see \
                `bynk.suite.duplicate_type` and `bynk.suite.duplicate_const`.",
        example: "// tests/desk.test.bynk\n\
                  fn twice(n: Int) -> Int { n * 2 }\n\
                  fn twice(n: Int) -> Int { n + n }     // ✗ `twice` again\n\
                  fn thrice(n: Int) -> Int { n * 3 }    // ✓",
        page: "/book/reference/testing",
        anchor: "helpers",
    },
    Explain {
        code: "bynk.suite.duplicate_type",
        blurb: "A suite's types are used by their bare names in its cases, so a suite declares each name once: a second `type Counted` would leave `Counted` meaning two shapes. Rename one. The same holds for a suite's constants, as `bynk.suite.duplicate_const`, and for helper functions, as `bynk.suite.duplicate_fn`.",
        example: "type Counted = { n: Int }\ntype Counted = { total: Int }     // ✗ `Counted` again\n\ntype Counted = { n: Int }\ntype Totalled = { total: Int }    // ✓ a name each",
        page: "/book/reference/testing",
        anchor: "suite-types-consts",
    },
    Explain {
        code: "bynk.suite.env_not_literal",
        blurb: "A Worker's variables and secrets are strings, so an `env` entry's value must be a `String` literal, `NAME = \"value\"`, or the name of a `fixture` the same suite declares. A number, a call or any other expression is refused, since the value is fixed when the suite is compiled. Write it as a string, and parse a number in the code that reads it.",
        example: "env {\n    RETRIES = 3       // ✗ not a `String`\n}\n\nenv {\n    RETRIES = \"3\"     // ✓\n}",
        page: "/book/reference/testing",
        anchor: "env",
    },
    Explain {
        code: "bynk.suite.fn_effectful",
        blurb: "A suite helper is a pure function that builds a value for a \
                case: a request, a fixture, an expected result. Returning an \
                `Effect` would hide a step of the test inside a helper. Build \
                the value in the helper and run the effect in the `case`, \
                where each step is visible with `<-`.",
        example: "// tests/desk.test.bynk\n\
                  fn bump(n: Int) -> Effect[Int] { local.call(n) }   // ✗ returns an `Effect`\n\
                  fn start() -> Int { 1 }                            // ✓ build the value\n\
                  let n <- local.call(start())                       // ✓ run it in the case",
        page: "/book/reference/testing",
        anchor: "helpers",
    },
    Explain {
        code: "bynk.suite.fn_method",
        blurb: "A suite helper is a plain function its cases call by name, so \
                it is a free `fn`, not a method (`fn Type.name`). A method \
                belongs to its type: declare it in the target or in a commons. \
                In a suite, take the value as an ordinary parameter.",
        example: "// demo/desk.bynk\n\
                  type Count = Int where NonNegative\n\n\
                  // tests/desk.test.bynk\n\
                  fn Count.bump(self) -> Int { 1 }       // ✗ a method\n\
                  fn bump(count: Count) -> Int { 1 }     // ✓ a free function",
        page: "/book/reference/testing",
        anchor: "helpers",
    },
    Explain {
        code: "bynk.suite.fn_name_clash",
        blurb: "A suite helper is called by bare name from the suite's cases, \
                which also see the target's declarations (services and agents \
                included) and those of the commons it `uses`. A helper with \
                one of those names would silently change which function a \
                call reaches, so it is refused instead of shadowing. A \
                fixture of the suite counts too, and `deps` and names \
                beginning `__` are reserved for the runner. Rename the helper. A type or constant clashing is \
                `bynk.suite.type_name_clash` or `bynk.suite.const_name_clash`.",
        example: "// demo/desk.bynk\n\
                  fn double(n: Int) -> Int { n * 2 }\n\n\
                  // tests/desk.test.bynk\n\
                  fn double(n: Int) -> Int { n * 3 }    // ✗ the target has a `double`\n\
                  fn triple(n: Int) -> Int { n * 3 }    // ✓",
        page: "/book/reference/testing",
        anchor: "helpers",
    },
    Explain {
        code: "bynk.suite.type_generic",
        blurb: "A suite's `type` is a plain shape that only its tests care \
                about, such as a row to read back with `select[T]`, so it \
                takes no type parameters. If you need a generic type, declare \
                it in the target or in a commons, where the suite can use it. \
                Otherwise write the concrete type.",
        example: "// tests/desk.test.bynk\n\
                  type Box[T] = { v: T }       // ✗ a suite type is not generic\n\
                  type IntBox = { v: Int }     // ✓",
        page: "/book/reference/testing",
        anchor: "suite-types-consts",
    },
    Explain {
        code: "bynk.suite.type_name_clash",
        blurb: "A suite's cases see the target's own declarations and those of the commons it `uses`, and the suite's types are used by bare name beside them. A suite type with one of those names would silently change which type a mention reaches, so it is refused instead of shadowing. The same goes for a variant of a suite's own sum type. Rename it. A constant clashing is `bynk.suite.const_name_clash`.",
        example: "// demo/desk.bynk\ntype Item = { sku: String }\n\n// tests/desk.test.bynk\ntype Item = { n: Int }       // ✗ the target already has an `Item`\ntype Counted = { n: Int }    // ✓ a name of its own",
        page: "/book/reference/testing",
        anchor: "suite-types-consts",
    },
    Explain {
        code: "bynk.suite.unknown_target",
        blurb: "A `suite` names the commons or context it tests, by its full \
                dotted name, and that unit must be declared in the project. A name \
                that matches none, usually a typo or a unit not written yet, would \
                leave the suite testing nothing. Correct the name, or declare the \
                unit.",
        example: "// demo/ledger.bynk\n\
                  commons demo.ledger\n\
                  \n\
                  // tests/ledger.test.bynk\n\
                  suite demo.ledgre { … }    // ✗ no such unit\n\
                  suite demo.ledger { … }    // ✓",
        page: "/book/reference/testing",
        anchor: "suite-blocks",
    },
    Explain {
        code: "bynk.test.actor_identity_required",
        blurb: "An actor that declares an `identity` stands for one particular \
                caller, and the handler reads that identity. A case that acts as \
                the actor must therefore say which caller, as in `by \
                User(\"bob\")`; a bare `by User` leaves the handler nothing to \
                read. A handler that binds its caller (`by c: Caller`) wants the \
                calling context's name the same way. Add the identity. The reverse \
                mistake is `bynk.test.actor_no_identity`.",
        example: "// demo/api.bynk\n\
                  type UserId = String where NonEmpty\n\
                  actor User { … }\n\n\
                  // tests/api.test.bynk\n\
                  let r <- api.GET(\"/a\") by User          // ✗ which user?\n\
                  let r <- api.GET(\"/a\") by User(\"bob\")   // ✓",
        page: "/book/reference/testing",
        anchor: "call-site-by",
    },
    Explain {
        code: "bynk.test.actor_no_identity",
        blurb: "A case supplies an identity only when the actor has one to give. \
                An actor that declares no `identity`, and the failure principals \
                `Nobody`, `Forged`, `Stale` and `Expired`, which present a missing \
                or bad credential and so no verified caller, take no argument. \
                Write `by Nobody`, not `by Nobody(\"bob\")`. For an actor that \
                does declare an identity and is given none, see \
                `bynk.test.actor_identity_required`.",
        example: "// tests/api.test.bynk\n\
                  let r <- api.GET(\"/a\") by Nobody(\"bob\")   // ✗ no credential, so no identity\n\
                  let r <- api.GET(\"/a\") by Nobody          // ✓",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.advance_not_clock",
        blurb: "`advance` moves the harness clock, the single time source a test \
                owns, so it names `Clock` and nothing else. No other capability \
                has virtual time to move. To shape what another capability \
                returns, use a `stub`. The clock only moves forward.",
        example: "// demo/clock.bynk\n\
                  consumes bynk { Clock }\n\
                  \n\
                  // tests/clock.test.bynk\n\
                  advance Random by 5.minutes   // ✗ only the Clock moves\n\
                  advance Clock by 5.minutes    // ✓",
        page: "/book/reference/testing",
        anchor: "time-in-tests",
    },
    Explain {
        code: "bynk.test.advance_not_duration",
        blurb: "`advance Clock by <amount>` moves the harness clock forward by a \
                `Duration`. A bare number does not say what unit it counts, so it \
                is refused. Write a duration literal such as `5.minutes`. See \
                `bynk.test.advance_not_clock` for naming a capability other than \
                `Clock`.",
        example: "// demo/clock.bynk\n\
                  consumes bynk { Clock }\n\
                  \n\
                  // tests/clock.test.bynk\n\
                  advance Clock by 5           // ✗ a number, not a Duration\n\
                  advance Clock by 5.minutes   // ✓",
        page: "/book/reference/testing",
        anchor: "time-in-tests",
    },
    Explain {
        code: "bynk.test.advance_outside_case",
        blurb: "`advance Clock by …` moves the harness clock, which only a \
                `case` owns. A `property` runs its body once per generated \
                input and has no clock of its own, and non-test code reads \
                real time. Put the time-dependent step in a `case`. \
                `bynk.test.advance_not_clock` and \
                `bynk.test.advance_not_duration` cover the other ways to get \
                `advance` wrong.",
        example: "property \"time moves on\" {\n\
                  \x20\x20for all n: Int {\n\
                  \x20\x20\x20\x20advance Clock by 5.minutes    // ✗ a property has no clock\n\
                  \x20\x20}\n\
                  }\n\n\
                  case \"time moves on\" {\n\
                  \x20\x20advance Clock by 5.minutes      // ✓\n\
                  }",
        page: "/book/reference/testing",
        anchor: "time-in-tests",
    },
    Explain {
        code: "bynk.test.asset_address_method",
        blurb: "An asset is served by the platform before the Worker runs, and only to `GET`: it answers every other method with `405`. A case fetches one with `api.GET(asset)`, and a different method is refused, since the answer would be the platform's `405` and nothing your code decides. Use `GET`, and pass the asset alone. A `by` clause on the fetch is `bynk.test.principal_on_asset`.",
        example: "// demo/web.bynk\nasset style = \"assets/board.css\"\n\n// tests/web.test.bynk\nlet r <- api.POST(style)    // ✗ only `GET` fetches an asset\nlet r <- api.GET(style)     // ✓",
        page: "/book/reference/testing",
        anchor: "assets",
    },
    Explain {
        code: "bynk.test.call_needs_in_process",
        blurb: "A service with an `on call` handler is internal to a deployment: \
                it has no public route, and a `system` case can only send requests \
                through the Worker's front door. So `svc.call(…)` is refused in \
                one. Call it at `unit` or `integration`, or drive an http route \
                that reaches it. For the event equivalent, see \
                `bynk.test.event_needs_in_process`.",
        example: "// demo/api.bynk\n\
                  service save { … }\n\n\
                  // tests/api.test.bynk\n\
                  case \"a key is measured\" as system {   // ✗ `save` has no public route\n\
                  case \"a key is measured\" {             // ✓\n\
                  \x20\x20\x20\x20let n <- save.call(\"abc\")\n\
                  }",
        page: "/book/reference/testing",
        anchor: "suite-blocks",
    },
    Explain {
        code: "bynk.test.event_needs_in_process",
        blurb: "`svc.event(e)` hands an event straight to a subscriber. At \
                `system`, an event reaches a subscriber only through its \
                publisher's fan-out across the deployed Workers, so there is no \
                direct delivery to make. Drive the handler that publishes the \
                event, or deliver the event at `unit` or `integration`. For the \
                `on call` equivalent, see `bynk.test.call_needs_in_process`.",
        example: "// shop/ledger.bynk\n\
                  service OnPlaced from Events(Placed) { … }\n\n\
                  // tests/ledger.test.bynk\n\
                  case \"a placed order is booked\" as system {   // ✗ no direct delivery at `system`\n\
                  case \"a placed order is booked\" {             // ✓\n\
                  \x20\x20\x20\x20let _ <- OnPlaced.event(Placed { id: \"o1\" })\n\
                  }",
        page: "/book/reference/testing",
        anchor: "event-address",
    },
    Explain {
        code: "bynk.test.event_not_subscribed",
        blurb: "`svc.event(e)` delivers an event through the handler the \
                service subscribes with, so it takes the one event named in \
                the service's `from Events(E)` header. Delivering a different \
                event would test a path the service does not have. Pass the \
                subscribed event, or test the service that does subscribe to \
                the one you meant.",
        example: "// shop/ledger.bynk\n\
                  service OnPlaced from Events(Placed) { … }\n\n\
                  // tests/ledger.test.bynk\n\
                  let _ <- OnPlaced.event(Cancelled { id: \"o1\" })   // ✗ it subscribes to `Placed`\n\
                  let _ <- OnPlaced.event(Placed { id: \"o1\" })      // ✓",
        page: "/book/reference/testing",
        anchor: "event-address",
    },
    Explain {
        code: "bynk.test.event_publisher_ambiguous",
        blurb: "`svc.event(e)` delivers an event in a default envelope, whose `publisherId` is the context that publishes the event to the target: its own, or the one context it consumes that declares it. When two consumed contexts both declare the event there is no single publisher to name, so the case must build the envelope itself. Pass `svc.event(e, env)`, with an `EventEnvelope` whose `publisherId` names the one you mean; the target must consume `EventEnvelope` from `bynk`.",
        example: "// shop/ledger.bynk\nconsumes shop.order\nconsumes shop.archive\nconsumes bynk { EventEnvelope }\n\n// tests/ledger.test.bynk\nlet _ <- OnPlaced.event(Placed { id: \"o1\" })         // ✗ `shop.order` or `shop.archive`?\nlet env = EventEnvelope { eventId: \"e1\", publisherId: \"shop.order\", …\nlet _ <- OnPlaced.event(Placed { id: \"o1\" }, env)    // ✓ the envelope names it",
        page: "/book/reference/testing",
        anchor: "event-address",
    },
    Explain {
        code: "bynk.test.expired_needs_bearer_route",
        blurb: "`by Expired` sends an expired token, so it needs a route whose \
                seam checks one: a `Bearer` or `Oidc` actor. A public route, or \
                one guarded by a `Signature` actor, has no token to expire; for a \
                `Signature` route `by Stale` sends a stale signature. Drive a \
                route guarded by a `Bearer` or `Oidc` actor. See \
                `bynk.test.nobody_needs_secured_route` for the same limit on \
                `Nobody`.",
        example: "// demo/api.bynk\n\
                  on GET(\"/a\") () -> Effect[HttpResult[Int]] by u: User { … }\n\
                  on GET(\"/health\") () -> Effect[HttpResult[Int]] by v: Visitor { … }\n\n\
                  // tests/api.test.bynk\n\
                  let r <- api.GET(\"/health\") by Expired   // ✗ a public route has no token\n\
                  let r <- api.GET(\"/a\") by Expired        // ✓",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.extra_headers_authorization",
        blurb: "A case sends a credential only through its `by` clause: the driver \
                builds the `Authorization` header (or the signature headers) from \
                the actor it acts as. Sending an `Authorization` header as an \
                extra request header, in `RequestHeaders.of`, would give the \
                request two competing credentials, so it is refused. Choose the \
                actor with `by`, and keep `RequestHeaders` for other headers.",
        example: "// tests/api.test.bynk\n\
                  let r <- api.GET(\"/health\", RequestHeaders.of([\"Authorization: Bearer x\"])) by Visitor   // ✗\n\
                  let r <- api.GET(\"/health\", RequestHeaders.of([\"X-Trace: 1\"])) by Visitor                // ✓",
        page: "/book/reference/testing",
        anchor: "request-headers",
    },
    Explain {
        code: "bynk.test.faults_needs_in_process",
        blurb: "`expect <call> faults` claims that a handler throws. At `system` a \
                fault reaches the case as an error response from the deployed \
                Worker, not a throw, so claim it at `unit` or `integration`, or \
                assert the response. A claim driven `by Nobody`, `Forged` or \
                `Stale` fails at every tier: only a `system` case reaches the auth \
                seam that refuses the request. Assert `Rejected(Unauthorized)` in \
                a `system` case instead.",
        example: "// tests/api.test.bynk\n\
                  expect api.GET(\"/a\") by Nobody faults     // ✗ a seam rejects, nothing throws\n\n\
                  case \"no token is refused\" as system {    // ✓\n\
                  \x20\x20\x20\x20let r <- api.GET(\"/a\") by Nobody\n\
                  \x20\x20\x20\x20expect r is Rejected(Unauthorized)\n\
                  }",
        page: "/book/reference/testing",
        anchor: "faults",
    },
    Explain {
        code: "bynk.test.forged_needs_signature_route",
        blurb: "`by Forged` and `by Stale` send a bad credential to a real auth seam, so the route must have a seam that can refuse it. `Forged` needs a `Signature`, `Bearer` or `Oidc` actor. `Stale` needs a `Signature` actor, since only a signature carries a timestamp to age. On any other route there is nothing to refuse and the case would test nothing, so it is refused. Drive a route with the right seam. See `bynk.test.stale_needs_timestamp` for a `Signature` actor with no timestamp.",
        example: "// demo/api.bynk\non GET(\"/me\") () -> Effect[HttpResult[String]] by u: User { … }\non POST(\"/hook\") (body: String) -> Effect[HttpResult[String]] by Hook { … }\n\n// tests/api.test.bynk\nlet r <- api.GET(\"/me\") by Stale             // ✗ a `Bearer` route has no signature to age\nlet r <- api.POST(\"/hook\", \"x\") by Stale     // ✓ `Hook` is a `Signature` actor",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.forged_needs_system",
        blurb: "`by Forged`, `by Stale` and the other bad-credential principals \
                send a forged or out-of-date credential to the real auth seam, \
                which only a `system` case reaches. In process the seam is skipped \
                and the identity is simply given, so there is nothing to refuse. \
                Move the case to `system` with `as system`.",
        example: "// tests/api.test.bynk\n\
                  case \"a forged token is refused\" {            // ✗ not a system case\n\
                  case \"a forged token is refused\" as system {  // ✓\n\
                  \x20\x20\x20\x20let r <- api.GET(\"/a\") by Forged\n\
                  \x20\x20\x20\x20expect r is Rejected(Unauthorized)\n\
                  }",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.no_subject_needs_oidc_route",
        blurb: "`by NoSubject` sends two correctly signed, unexpired tokens whose `sub` the seam must refuse: one empty, one missing. Only an `Oidc` actor's seam is driven with these tokens, so on any other route, a `Bearer` or `Signature` one or a public one, the case is refused. Drive a route guarded by an `Oidc` actor. The same holds for `bynk.test.wrong_issuer_needs_oidc_route`, `bynk.test.wrong_audience_needs_oidc_route` and `bynk.test.wrong_algorithm_needs_oidc_route`.",
        example: "// demo/api.bynk\nactor User { auth = Bearer(secret = \"S\"), identity = UserId }\nactor Workload { auth = Oidc(…\non GET(\"/me\") () -> Effect[HttpResult[String]] by u: User { … }\non GET(\"/who\") () -> Effect[HttpResult[String]] by w: Workload { … }\n\n// tests/api.test.bynk\nlet r <- api.GET(\"/me\") by NoSubject      // ✗ a `Bearer` route is not an `Oidc` one\nlet r <- api.GET(\"/who\") by NoSubject     // ✓",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.nobody_needs_secured_route",
        blurb: "`by Nobody` sends no credential so that a route's auth seam \
                answers `401`. A public route, guarded by `Visitor`, has no seam \
                that refuses anyone, so the case would test nothing and is \
                refused. Use `by Nobody` on a route guarded by a `Bearer`, `Oidc` \
                or `Signature` actor, and act as `Visitor` on a public one. For \
                the other failure principals, see \
                `bynk.test.expired_needs_bearer_route`.",
        example: "// demo/api.bynk\n\
                  on GET(\"/health\") () -> Effect[HttpResult[Int]] by v: Visitor { … }\n\n\
                  // tests/api.test.bynk\n\
                  let r <- api.GET(\"/health\") by Nobody     // ✗ a public route has no seam\n\
                  let r <- api.GET(\"/health\") by Visitor    // ✓",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.principal_claims_not_oidc",
        blurb: "`TokenClaims.of([…])` supplies the claims an `Oidc` actor's token \
                carries, which the handler reads and the actor's rules check. A \
                `Bearer` actor's token holds an identity and nothing else, so \
                there is no claim set to fill, and the argument is refused. Drop \
                the claims, or test a route guarded by an `Oidc` actor. See \
                `bynk.test.actor_no_identity` for the other extra argument.",
        example: "// tests/api.test.bynk\n\
                  let r <- api.GET(\"/me\") by User(\"bob\", TokenClaims.of([\"repository: acme/money\"]))   // ✗ a Bearer actor\n\
                  let r <- api.GET(\"/me\") by User(\"bob\")                                               // ✓",
        page: "/book/reference/testing",
        anchor: "token-claims",
    },
    Explain {
        code: "bynk.test.principal_identity_mismatch",
        blurb: "A handler guarded by an actor runs as that actor, so a case must \
                act as it, with the identity type it declares. `by Visitor` \
                carries no identity, so it cannot stand in for `User`, and an \
                actor outside the handler's `by` sum is no member of it. Act as \
                the actor the handler names. To exercise an unauthenticated \
                request instead, use `by Nobody` in a `system` case.",
        example: "// demo/api.bynk\n\
                  actor User { … }\n\n\
                  // tests/api.test.bynk\n\
                  let r <- api.GET(\"/a\") by Visitor        // ✗ the handler is `by u: User`\n\
                  let r <- api.GET(\"/a\") by User(\"bob\")    // ✓",
        page: "/book/reference/testing",
        anchor: "call-site-by",
    },
    Explain {
        code: "bynk.test.principal_on_asset",
        blurb: "An asset is public: the platform serves it before the Worker runs, so no actor's seam ever sees the request. A `by` clause on an asset fetch would name a principal that nothing checks, so it is refused. Drop the `by`. To test who may reach something, drive a route, not an asset. A method other than `GET` is `bynk.test.asset_address_method`.",
        example: "// demo/web.bynk\nasset style = \"assets/board.css\"\n\n// tests/web.test.bynk\nlet r <- api.GET(style) by Visitor    // ✗ an asset has no principal\nlet r <- api.GET(style)               // ✓",
        page: "/book/reference/testing",
        anchor: "assets",
    },
    Explain {
        code: "bynk.test.principal_on_wrong_method",
        blurb: "A wrong-method test, such as `DELETE` on a path declared only for \
                `GET`, expects the `405` the router returns before any handler is \
                chosen. No handler runs, so there is no actor to act as, and a \
                `by` clause would suggest otherwise. Drop the `by` clause. To test \
                who may call a route, address a method it does declare.",
        example: "// tests/api.test.bynk\n\
                  let r <- api.DELETE(\"/a\") by User(\"bob\")   // ✗ no handler runs\n\
                  let r <- api.DELETE(\"/a\")                  // ✓\n\
                  expect r is Rejected(MethodNotAllowed)",
        page: "/book/reference/testing",
        anchor: "wrong-method",
    },
    Explain {
        code: "bynk.test.principal_required",
        blurb: "A handler declared `by u: User` runs as a verified actor, and \
                a `case` has no real caller, so it must say who is calling \
                with a call-site `by`. The same applies to a handler guarded \
                by an actor sum: name one of its members. Append `by \
                <Actor>(<identity>)` to the call.",
        example: "// tests/api.test.bynk\n\
                  let r <- api.GET(\"/items\")                  // ✗ who is calling?\n\
                  let r <- api.GET(\"/items\") by User(\"ada\")   // ✓",
        page: "/book/reference/testing",
        anchor: "call-site-by",
    },
    Explain {
        code: "bynk.test.service_bad_address",
        blurb: "A test addresses an http route by method and route pattern, as in \
                `api.POST(\"/items\", body)`. The first argument must be that \
                pattern as a string literal, so a number or a variable is refused. \
                The same code covers other addresses used the wrong way, such as \
                fetching an asset through a service that is not `from http`.",
        example: "// demo/api.bynk\n\
                  service api from http { … }\n\
                  \n\
                  // tests/api.test.bynk\n\
                  let r <- api.POST(1, \"x\") by Visitor          // ✗ not a pattern\n\
                  let r <- api.POST(\"/items\", \"x\") by Visitor   // ✓",
        page: "/book/reference/testing",
        anchor: "suite-blocks",
    },
    Explain {
        code: "bynk.test.service_call_arity",
        blurb: "`svc.call(…)` in a test runs the service's `on call` handler, so \
                it takes that handler's parameters, and the number of arguments is \
                checked as for any call. Add or remove arguments to match the \
                handler. The asset address, `svc.GET(asset)`, takes the asset \
                alone, and this code covers extra arguments there too.",
        example: "// demo/svc.bynk\n\
                  service echo { … }\n\
                  \n\
                  // tests/svc.test.bynk\n\
                  let r <- echo.call(1, 2)   // ✗ the handler takes one\n\
                  let r <- echo.call(1)      // ✓",
        page: "/book/reference/testing",
        anchor: "suite-blocks",
    },
    Explain {
        code: "bynk.test.service_no_call_handler",
        blurb: "A case reaches a service through the address its protocol has. \
                `svc.call(…)` runs an `on call` handler, so it needs one; a `from \
                http` service is driven by route (`api.POST(\"/items\", body)`), a \
                `from cron` service by its schedule, a `from queue` service by \
                message. Use the address the service has. The Book's table of \
                addresses lists each.",
        example: "// demo/api.bynk\n\
                  service api from http { … }\n\
                  \n\
                  // tests/api.test.bynk\n\
                  let r <- api.call(\"x\")                      // ✗ no `on call` handler\n\
                  let r <- api.POST(\"/items\", \"x\") by Visitor // ✓ its route",
        page: "/book/reference/testing",
        anchor: "suite-blocks",
    },
    Explain {
        code: "bynk.test.service_unknown_route",
        blurb: "A case addresses an http route by the pattern its handler \
                declares, `\"/items\"`, not a concrete URL, and a cron or queue \
                service by its own address. A path that no route in the service \
                declares, for any method, is refused. To test the `405` for a \
                wrong method, use a declared path with a method it lacks.",
        example: "// demo/api.bynk\n\
                  service api from http { … }\n\
                  \n\
                  // tests/api.test.bynk\n\
                  let r <- api.POST(\"/item\", \"x\") by Visitor    // ✗ no route \"/item\"\n\
                  let r <- api.POST(\"/items\", \"x\") by Visitor   // ✓",
        page: "/book/reference/testing",
        anchor: "suite-blocks",
    },
    Explain {
        code: "bynk.test.snapshot_duplicate_name",
        blurb: "A snapshot is stored under its case and its name, so each \
                `matches snapshot` in one case needs a name of its own. Two \
                with the same name would compare against, and update, the same \
                stored value. Give the repeat a distinct name.",
        example: "expect read(\"a\") matches snapshot \"delivery\"\n\
                  expect read(\"b\") matches snapshot \"delivery\"   // ✗ name already taken\n\
                  expect read(\"b\") matches snapshot \"second\"     // ✓",
        page: "/book/reference/testing",
        anchor: "snapshots",
    },
    Explain {
        code: "bynk.test.snapshot_in_property",
        blurb: "A snapshot is keyed by its case and name, so it holds one value. A \
                `property` runs its body once for each generated input, so there \
                is no single value to store. Snapshot a fixed value in a `case`, \
                and let the property assert what holds for every input.",
        example: "// tests/api.test.bynk\n\
                  property \"every n\" {\n\
                  \x20\x20\x20\x20for all n: Int {\n\
                  \x20\x20\x20\x20\x20\x20\x20\x20expect run(n) matches snapshot \"n\"   // ✗ many inputs, one snapshot\n\
                  \x20\x20\x20\x20}\n\
                  }\n\n\
                  case \"one n\" {\n\
                  \x20\x20\x20\x20expect run(3) matches snapshot \"n\"       // ✓\n\
                  }",
        page: "/book/reference/testing",
        anchor: "snapshots",
    },
    Explain {
        code: "bynk.test.snapshot_name_not_literal",
        blurb: "A snapshot is stored under its name, so the name must be a \
                string literal: the compiler then knows every snapshot a suite \
                takes. A variable or other expression is refused. Write the \
                name as a literal.",
        example: "expect read(\"a\") matches snapshot name        // ✗ not a string literal\n\
                  expect read(\"a\") matches snapshot \"first\"     // ✓",
        page: "/book/reference/testing",
        anchor: "snapshots",
    },
    Explain {
        code: "bynk.test.snapshot_needs_unit_suite",
        blurb: "A snapshot is stored in a file beside its unit suite and compared \
                by the unit harness, so `matches snapshot` works only in a \
                unit-tier `case` of a suite that tests a unit. A `system` case, or \
                an integration or system suite, has no store to compare against. \
                Take the snapshot in a unit case. See \
                `bynk.test.snapshot_in_property` for the other place it is \
                refused.",
        example: "// tests/api.test.bynk\n\
                  suite demo.api as system {\n\
                  \x20\x20\x20\x20case \"one n\" {\n\
                  \x20\x20\x20\x20\x20\x20\x20\x20expect 3 matches snapshot \"n\"   // ✗ no snapshot store at `system`\n\
                  \x20\x20\x20\x20}\n\
                  }\n\n\
                  suite demo.api {                             // ✓ a unit suite\n\
                  \x20\x20\x20\x20case \"one n\" {\n\
                  \x20\x20\x20\x20\x20\x20\x20\x20expect 3 matches snapshot \"n\"\n\
                  \x20\x20\x20\x20}\n\
                  }",
        page: "/book/reference/testing",
        anchor: "snapshots",
    },
    Explain {
        code: "bynk.test.snapshot_not_codable",
        blurb: "A snapshot stores the value as the JSON text `Json.encode` \
                produces, so the value must be `Codable`: base types, named \
                types and the built-in containers over them. A `Stream`, a \
                function, an `Effect` or `()` has no such text. Snapshot a \
                `Codable` value instead, such as a list.",
        example: "expect Stream.of([1, 2]) matches snapshot \"items\"   // ✗ a `Stream`\n\
                  expect [1, 2] matches snapshot \"items\"              // ✓ a `List[Int]`",
        page: "/book/reference/testing",
        anchor: "snapshots",
    },
    Explain {
        code: "bynk.test.stale_needs_timestamp",
        blurb: "A `Signature` actor refuses a stale request by reading a timestamp header and comparing it with a `tolerance`. `by Stale` signs a request in the past, so it means something only when the actor declares both; without them the seam has no window to refuse it by, and the case would test nothing. Add `timestamp = \"<Header>\"` and `tolerance = <seconds>` to the actor, or test the signature itself with `by Forged`. On a route with no `Signature` actor, see `bynk.test.forged_needs_signature_route`.",
        example: "// demo/api.bynk\nactor Legacy { auth = Signature(secret = \"LEGACY_SECRET\", header = \"X-Legacy-Sig\") }\nactor Hook { auth = Signature(secret = \"HOOK_SECRET\", header = \"X-Sig\", timestamp = \"X-Ts\", tolerance = 300) }\n\n// tests/api.test.bynk\nlet r <- api.POST(\"/legacy\", \"x\") by Stale    // ✗ `Legacy` declares no `timestamp`\nlet r <- api.POST(\"/hook\", \"x\") by Stale      // ✓",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.system_claims_need_literal",
        blurb: "At `system` the driver mints an `Oidc` actor's token itself, writing its claims from the `\"name: value\"` lines of the `TokenClaims.of([…])` in the `by` clause. A `TokenClaims` held in a variable is opaque: it can be read with `get` and `has`, but its lines cannot be recovered, so the driver has nothing to mint from and the case is refused rather than sent with no claims. Write `TokenClaims.of([…])` in the principal, or move the case to `integration`, where the handler is given the value as it is.",
        example: "// tests/api.test.bynk\nlet claims = TokenClaims.of([\"repository: acme/money\"])\nlet r <- api.GET(\"/who\") by Workload(\"wid\", claims)    // ✗ the claims are in a variable\nlet r <- api.GET(\"/who\") by Workload(\"wid\", TokenClaims.of([\"repository: acme/money\"]))    // ✓",
        page: "/book/reference/testing",
        anchor: "token-claims",
    },
    Explain {
        code: "bynk.test.unknown_actor",
        blurb: "A call-site `by <Actor>` makes the case act as that actor, so \
                it must name an `actor` the target context declares, or one of \
                the prelude actors such as `Visitor` and `Caller`. Any other \
                name, often a misspelling, has no identity to supply. Correct \
                the name or declare the actor.",
        example: "// demo/api.bynk\n\
                  actor User { auth = Bearer(secret = \"S\"), identity = UserId }\n\n\
                  // tests/api.test.bynk\n\
                  let r <- api.GET(\"/items\") by Ghost(\"x\")   // ✗ no actor `Ghost`\n\
                  let r <- api.GET(\"/items\") by User(\"x\")    // ✓",
        page: "/book/reference/testing",
        anchor: "call-site-by",
    },
    Explain {
        code: "bynk.test.wire_http_only",
        blurb: "`Wire(…)` delivers raw text to an http body or path segment. A \
                queue message, a cron tick, an `on call` or an event has no raw \
                text form: it is a typed value, and `Wire` would hand it to a \
                typed decoder as a string and fail confusingly at run time. Pass \
                the typed value. To drive an http route with raw text, see \
                `bynk.test.wire_needs_system`.",
        example: "// demo/api.bynk\n\
                  service packer from queue(\"packing\") { … }\n\n\
                  // tests/api.test.bynk\n\
                  let r <- packer.message(Wire(\"{}\"))   // ✗ a queue takes a typed message\n\
                  let r <- packer.message(\"{}\")         // ✓",
        page: "/book/reference/testing",
        anchor: "driving-a-rejection-with-wire",
    },
    Explain {
        code: "bynk.test.wire_needs_system",
        blurb: "`Wire(…)` hands raw text to the real boundary, before any \
                decoding, so that a case can check what the boundary refuses. Only \
                a `system` case has a real boundary: below it the handler is \
                called directly, and the text would go in unchecked. Promote the \
                case with `as system`, or pass a typed value instead. For other \
                addresses, see `bynk.test.wire_http_only`.",
        example: "// tests/api.test.bynk\n\
                  case \"raw text is refused\" {            // ✗ not a system case\n\
                  case \"raw text is refused\" as system {  // ✓\n\
                  \x20\x20\x20\x20let r <- api.POST(\"/n\", Wire(\"0\")) by Visitor\n\
                  \x20\x20\x20\x20expect r is Rejected(RefinementViolation(_))\n\
                  }",
        page: "/book/reference/testing",
        anchor: "driving-a-rejection-with-wire",
    },
    Explain {
        code: "bynk.test.wrong_algorithm_needs_oidc_route",
        blurb: "`by WrongAlgorithm` sends two otherwise valid tokens under an algorithm the seam must refuse: one with `alg: none`, and one `HS256`-signed with the JWKS public key as the secret. Only an `Oidc` actor's seam is driven with these tokens, so on any other route, a `Bearer` or `Signature` one or a public one, the case is refused. Drive a route guarded by an `Oidc` actor. The same holds for `bynk.test.wrong_issuer_needs_oidc_route`, `bynk.test.wrong_audience_needs_oidc_route` and `bynk.test.no_subject_needs_oidc_route`.",
        example: "// demo/api.bynk\nactor User { auth = Bearer(secret = \"S\"), identity = UserId }\nactor Workload { auth = Oidc(…\non GET(\"/me\") () -> Effect[HttpResult[String]] by u: User { … }\non GET(\"/who\") () -> Effect[HttpResult[String]] by w: Workload { … }\n\n// tests/api.test.bynk\nlet r <- api.GET(\"/me\") by WrongAlgorithm      // ✗ a `Bearer` route is not an `Oidc` one\nlet r <- api.GET(\"/who\") by WrongAlgorithm     // ✓",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.wrong_audience_needs_oidc_route",
        blurb: "`by WrongAudience` sends a correctly signed, unexpired token whose `aud` is not the actor's audience. Only an `Oidc` actor has an audience to check: a `Bearer` token has none, and a `Signature` or public route has no token. On any other route the case is refused. Drive a route guarded by an `Oidc` actor. The same holds for `bynk.test.wrong_issuer_needs_oidc_route`, `bynk.test.wrong_algorithm_needs_oidc_route` and `bynk.test.no_subject_needs_oidc_route`.",
        example: "// demo/api.bynk\nactor User { auth = Bearer(secret = \"S\"), identity = UserId }\nactor Workload { auth = Oidc(…\non GET(\"/me\") () -> Effect[HttpResult[String]] by u: User { … }\non GET(\"/who\") () -> Effect[HttpResult[String]] by w: Workload { … }\n\n// tests/api.test.bynk\nlet r <- api.GET(\"/me\") by WrongAudience      // ✗ a `Bearer` token has no audience\nlet r <- api.GET(\"/who\") by WrongAudience     // ✓",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.wrong_issuer_needs_oidc_route",
        blurb: "`by WrongIssuer` sends a correctly signed, unexpired token whose `iss` is not the actor's issuer. Only an `Oidc` actor has an issuer to check: a `Bearer` token has none, and a `Signature` or public route has no token. On any other route the case is refused. Drive a route guarded by an `Oidc` actor. The same holds for `bynk.test.wrong_audience_needs_oidc_route`, `bynk.test.wrong_algorithm_needs_oidc_route` and `bynk.test.no_subject_needs_oidc_route`.",
        example: "// demo/api.bynk\nactor User { auth = Bearer(secret = \"S\"), identity = UserId }\nactor Workload { auth = Oidc(…\non GET(\"/me\") () -> Effect[HttpResult[String]] by u: User { … }\non GET(\"/who\") () -> Effect[HttpResult[String]] by w: Workload { … }\n\n// tests/api.test.bynk\nlet r <- api.GET(\"/me\") by WrongIssuer      // ✗ a `Bearer` token has no issuer\nlet r <- api.GET(\"/who\") by WrongIssuer     // ✓",
        page: "/book/reference/testing",
        anchor: "failure-principals",
    },
    Explain {
        code: "bynk.test.ws_lifecycle_by",
        blurb: "A WebSocket connection's principal is given once, at `open`, and the connection carries it from there, as the platform recovers it from the socket. So `message` and `close` take no `by`: a case that named one could present a principal production never sees. Drop the `by`. To act as another principal, open a new `TestConnection`.",
        example: "// tests/room.test.bynk\nlet tc = TestConnection[Frame]()\nlet _ <- Gateway.open(tc, RoomId.unsafe(\"r\")) by Member(UserId.unsafe(\"ann\"))\nlet _ <- Gateway.message(tc, Frame { text: \"hi\" }) by Member(UserId.unsafe(\"ann\"))    // ✗ given at `open`\nlet _ <- Gateway.message(tc, Frame { text: \"hi\" })                                    // ✓",
        page: "/book/reference/testing",
        anchor: "websocket-address",
    },
    Explain {
        code: "bynk.test.ws_needs_in_process",
        blurb: "`svc.open(tc, …)`, `svc.message` and `svc.close` drive a `from websocket` service in process, against a `TestConnection`. A deployed WebSocket is a real upgrade through the Worker and a Durable Object, which a `system` case does not drive, so the address has nothing to run. Drive the service in a case at `unit` or `integration` instead.",
        example: "// tests/room.test.bynk\ncase \"a join\" as system {         // ✗ a `system` case cannot drive a WebSocket\ncase \"a join\" as integration {    // ✓\n    let tc = TestConnection[Frame]()\n    let _ <- Gateway.open(tc, Val[RoomId]) by Member(Val[UserId])\n}",
        page: "/book/reference/testing",
        anchor: "websocket-address",
    },
    Explain {
        code: "bynk.tier.cross_context_needs_system",
        blurb: "Below `system` a case runs in one process. No other context is stood up, and a `stub` doubles capabilities, not another context's services. So a `unit` or `integration` case, or a `property`, that reaches another context, directly or through a service or agent handler, has nothing to call. Promote the case to a `system` suite, which wires the contexts, or test the unit that does not cross.",
        example: "suite demo.desk {\n    case \"the desk checks\" {\n        let n <- check.call(5)   // ✗ `check` calls `demo.vault`, another context\n    }\n}\n\nsuite demo.desk as system {\n    case \"the desk checks\" {\n        let n <- demo.desk.check(5)   // ✓ both contexts are stood up\n    }\n}",
        page: "/book/reference/testing",
        anchor: "tiers-the-as-tier-clause",
    },
    Explain {
        code: "bynk.tier.mixed_system_suite",
        blurb: "A `system` case runs against deployed Workers and addresses services by their context path, which no `unit` or `integration` case can. So a suite holds either `system` cases only or none: a case of another tier in a `system` suite, or a `system` case among others, is refused. Move it into a suite of its own.",
        example: "suite demo.desk as system {\n    case \"a local one\" as unit {   // ✗ not `system`, in a `system` suite\n        let n <- local.call(1)\n    }\n}\n\nsuite demo.desk {\n    case \"a local one\" {   // ✓ in a suite of its own\n        let n <- local.call(1)\n    }\n}",
        page: "/book/reference/testing",
        anchor: "tiers-the-as-tier-clause",
    },
    Explain {
        code: "bynk.tier.system_needs_wire",
        blurb: "A `system` case exists to cross a real serialise, JSON, deserialise boundary. A target has one if it consumes another context, or exposes an `http`, `queue` or `email` service. One with neither has nothing to cross, so a `system` suite adds nothing over `unit`. Run the suite at `unit` or `integration`, or give the target a service that has a boundary.",
        example: "suite commerce.payment as system {   // ✗ no other context, and no `http`, `queue` or `email` service\n    case \"authorises\" { … }\n}\n\nsuite commerce.payment as integration {   // ✓ real collaborators, no wire\n    case \"authorises\" { … }\n}",
        page: "/book/reference/testing",
        anchor: "tiers-the-as-tier-clause",
    },
    Explain {
        code: "bynk.transition.cross_agent_reference",
        blurb: "A transition is checked inside one agent's commit, between that agent's own old and new states. Reading another agent would mean a call out to it, whose value could change before the commit lands, so the claim could not be held. A property that spans agents belongs in a saga or a scenario. Keep the transition to this agent's `old` and `new`. The mirror rule for states is `bynk.invariant.cross_agent_reference`.",
        example: "transition within_total: new.count <= Total(\"all\").get()   // ✗ reads another agent\n\ntransition grows: new.count >= old.count                   // ✓ this agent's own step",
        page: "/book/reference/agent-invariants",
        anchor: "step-invariants",
    },
    Explain {
        code: "bynk.transition.duplicate_name",
        blurb: "Transitions are named so that a violation can say which step rule was broken. Two with the same name in one agent would make that report ambiguous. Rename one, or merge the two predicates if they are one claim. The same rule for states is `bynk.invariant.duplicate_name`.",
        example: "transition grows: new.count >= old.count\ntransition grows: new.count < 100    // ✗ `grows` is already declared\n\ntransition capped: new.count < 100   // ✓ a distinct name",
        page: "/book/reference/agent-invariants",
        anchor: "step-invariants",
    },
    Explain {
        code: "bynk.transition.impure_predicate",
        blurb: "A transition is checked at each commit, over the old and new states, so it may only read: `old`, `new`, operators and pure value methods. `Effect.pure`, `?`, `expect`, `Val` and the other test-only forms are refused, as is anything that performs an effect. Rewrite the predicate as a plain `Bool` expression. The mirror rule for states is `bynk.invariant.impure_predicate`.",
        example: "transition grows: Effect.pure(new.count >= old.count)   // ✗ an effect, not a pure predicate\n\ntransition grows: new.count >= old.count                // ✓ a plain `Bool` expression",
        page: "/book/reference/agent-invariants",
        anchor: "step-invariants",
    },
    Explain {
        code: "bynk.transition.no_step_reference",
        blurb: "A transition constrains a step, so it has to compare the state before the commit (`old`) with the state after (`new`). One that mentions neither is a claim about a single state, which an `invariant` states more plainly and checks the same way. Use `old` and `new`, or write it as an `invariant`.",
        example: "transition grows: count >= 0               // ✗ names neither `old` nor `new`\n\ntransition grows: new.count >= old.count   // ✓ compares the two states",
        page: "/book/reference/agent-invariants",
        anchor: "step-invariants",
    },
    Explain {
        code: "bynk.transition.not_bool",
        blurb: "A `transition` is a claim about a step, from `old` to `new`, that is either kept or broken, so its predicate must be a `Bool`. A bare `new.count` is a value, not a claim. Compare it with the old state: `new.count >= old.count`. An `invariant` is held to the same rule (`bynk.invariant.not_bool`).",
        example: "transition grows: new.count                // ✗ an `Int`, not a `Bool`\n\ntransition grows: new.count >= old.count   // ✓ a claim about the step",
        page: "/book/reference/agent-invariants",
        anchor: "step-invariants",
    },
    Explain {
        code: "bynk.types.argument_mismatch",
        blurb: "An argument has a different type from the parameter it is passed to. Bynk \
                compares types exactly and converts nothing implicitly, so a `String` is \
                never read as an `Int`, and a call that passes the wrong type is refused \
                at the call, not left to fail later. The same code covers the arguments of \
                methods, capability operations and constructors. Pass a value of the \
                parameter's type, or convert it first with an explicit conversion such as \
                `Int.parse` (which returns an `Option`, because the text may not be a number).",
        example: "fn double(n: Int) -> Int {\n    n * 2\n}\n\n\
                  fn run() -> Int {\n    double(\"21\")    // ✗ a `String`, where `Int` is expected\n}\n\n\
                  fn run() -> Int {\n    double(21)      // ✓ an `Int`\n}",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.types.asset_at_boundary",
        blurb: "An `Asset` names a file the platform serves; it is not data to keep or send. So an \
                `Asset` cannot be a record field, a sum payload, an event field, an agent key or \
                store field, or part of a service or agent handler signature. What crosses is its \
                `.url`, a `String`: hold that instead. Other types refused there include \
                `bynk.types.function_at_boundary` and `bynk.types.held_at_boundary`.",
        example: "type Page = {\n    title: String,\n    style: Asset,     // ✗ an asset is not a value to hold\n}\n\ntype Page = {\n    title: String,\n    style: String,    // ✓ keep its `.url`\n}",
        page: "/book/reference/assets",
        anchor: "the-url",
    },
    Explain {
        code: "bynk.types.bad_field_default",
        blurb: "A field default, `field: T = expr`, is filled in whenever a literal leaves the \
                field out and whenever a decoded body omits its key, so it has to be a fixed \
                value that has a wire form. The compiler accepts a literal, a variant, \
                `Some`, `None`, `Ok` or `Err`, a record or list literal, `Map.empty()`, \
                `Set.empty()`, and `T.unsafe(lit)` for an opaque type. A call to your own \
                function, or a read of a binding, is refused: a default has to be a value known \
                when the program compiles, not something computed at every construction \
                and decode. In a generic record, a field whose type is the type parameter itself cannot have a \
                default at all, since one value cannot suit every instantiation. Write the \
                value out as a literal, or drop the default and require the field.",
        example: "fn three() -> Int { 3 }\n\n\
                  type Config = {\n  retries: Int = three(),    // ✗ a call is not a static value\n}\n\n\
                  type Config = {\n  retries: Int = 3,          // ✓ a literal\n}",
        page: "/book/reference/types",
        anchor: "field-defaults",
    },
    Explain {
        code: "bynk.types.call_arity",
        blurb: "A function value was called with a different number of arguments from the \
                number its type declares. A parameter or local of type `(Int, Int) -> Int` \
                takes exactly two, and the call is checked against that type, so the \
                mismatch is reported where it is made. The same code covers the built-in \
                operations of `Map`, `Set`, `Cell`, `Cache` and `Log`, and `unreachable` and \
                `todo`, which each take one message. Pass exactly the declared number. A \
                declared `fn` called with the wrong number of arguments is the separate code \
                `bynk.resolve.arity_mismatch`, and a method is `bynk.types.method_arity`.",
        example: "fn apply(f: (Int, Int) -> Int) -> Int {\n    f(1)       // ✗ `f` takes 2 arguments\n}\n\n\
                  fn apply(f: (Int, Int) -> Int) -> Int {\n    f(1, 2)    // ✓\n}",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.types.cannot_infer_option_type_param",
        blurb: "`None` carries no value, so its `Option[T]` takes `T` from where it is used: an annotation, a parameter, or the function's return type. A `let` of a bare `None` has none, and the compiler will not guess, because `T` becomes part of everything the binding touches. Annotate it, as in `let missing: Option[Int] = None`, or use `None` where an `Option` is expected.",
        example: "fn run() -> Int {\n    let missing = None    // ✗ an `Option` of what?\n    1\n}\n\nfn run() -> Int {\n    let missing: Option[Int] = None    // ✓\n    1\n}",
        page: "/book/guides/type-system/result-and-optionals",
        anchor: "construct-values",
    },
    Explain {
        code: "bynk.types.cannot_infer_result_type_params",
        blurb: "`Ok(v)` fixes only the success type and `Err(e)` only the error type, so the rest of the `Result` has to come from context: an annotation, a parameter type, or the enclosing function's return type. A bare `let r = Ok(1)` has neither, and the compiler will not guess. Annotate the binding, as in `let r: Result[Int, String] = Ok(1)`.",
        example: "fn run() -> Int {\n    let r = Ok(1)    // ✗ a `Result` of what error?\n    1\n}\n\nfn run() -> Int {\n    let r: Result[Int, String] = Ok(1)    // ✓\n    1\n}",
        page: "/book/guides/type-system/result-and-optionals",
        anchor: "construct-values",
    },
    Explain {
        code: "bynk.types.catastrophic_regex",
        blurb: "A `Matches` pattern, and the pattern of a regex method such as \
                `matchesRegex` or `replaceRegex`, is analysed when the program compiles. \
                Patterns run with JavaScript's backtracking `RegExp`, and one in which a \
                repetition can split the same text several ways takes exponential time on a \
                crafted near miss, so a few dozen characters of input could stall a handler. \
                Nested or ambiguous repetition, such as `(a+)+` or `(a|aa)+`, is refused, \
                bounded or not, as is a backreference under an unbounded quantifier, an unbounded quantifier over a \
                body that can match the empty string, and a pattern too large or complex to \
                analyse. Rewrite it so each repetition has one way to match: `a+` for `(a+)+`, \
                or a separator the repeated part cannot match, as in `[a-z]+(-[a-z]+)*`. \
                Two adjacent repetitions that compete, as in `\\d*\\d*`, are the milder \
                `bynk.types.polynomial_regex`.",
        example: "type Code = String where Matches(\"(a+)+$\")    // ✗ nested unbounded repetition\n\n\
                  type Code = String where Matches(\"a+$\")       // ✓ one way to match",
        page: "/book/reference/types",
        anchor: "regular-expressions",
    },
    Explain {
        code: "bynk.types.claims_at_boundary",
        blurb: "An `Oidc` actor's binder carries the verified token's claims (`who.claims`) for the \
                handler that received the token. They are read there and never kept: a \
                `TokenClaims` can't be a field, or cross a call, a message or an agent. Read the \
                claim you need and keep that `String`.",
        example: "type Seen = { claims: TokenClaims }                 // ✗\n\n\
                  let repo = w.claims.get(\"repository\")            // ✓ keep the String",
        page: "/book/reference/actors",
        anchor: "reading-claims",
    },
    Explain {
        code: "bynk.types.combinator_return_mismatch",
        blurb: "The function given to a combinator must return the shape the combinator \
                chains on. `Option.andThen` wants an `Option` back and `Result.andThen` a \
                `Result` whose error type matches the receiver's, so that the result is \
                flattened, not nested. `List.flatMap` wants a `List` and `Query.flatMap` a \
                `Query`. The effectful iterators and `flatMapOk` and `flatMapErr` want an \
                `Effect[Result[…]]`, and `Option.orElse` wants the receiver's own `Option` \
                type. If your function returns a plain value, you want `map`, which wraps it \
                for you. Otherwise wrap the value yourself, as `Some(n + 1)`, or fix the \
                function's result. A callback of the wrong type for the combinator's own \
                signature is `bynk.types.argument_mismatch`.",
        example: "fn bump(o: Option[Int]) -> Option[Int] {\n    o.andThen((n) => n + 1)    // ✗ must return an `Option`, not an `Int`\n}\n\n\
                  fn bump(o: Option[Int]) -> Option[Int] {\n    o.map((n) => n + 1)        // ✓ `map` wraps a plain value\n}",
        page: "/book/reference/types",
        anchor: "built-in-generic-types",
    },
    Explain {
        code: "bynk.types.constructor_arity",
        blurb: "`T.of(value)` takes exactly one argument, the base value to check, and so does `T.unsafe(value)` on an opaque type: a refined or opaque type wraps a single value. None gives it nothing to check, and two have no meaning. Pass the one value. Building a sum variant with the wrong number of arguments is `bynk.types.variant_arity`.",
        example: "type Age = Int where InRange(0, 150)\n\nfn parse(n: Int) -> Result[Age, ValidationError] {\n    Age.of(n, 1)    // ✗ `of` takes 1 argument\n}\n\nfn parse(n: Int) -> Result[Age, ValidationError] {\n    Age.of(n)    // ✓\n}",
        page: "/book/reference/refined-types",
        anchor: "",
    },
    Explain {
        code: "bynk.types.constructor_base_mismatch",
        blurb: "`Reps.of(x)` checks a value of the type's base against its predicates, so `x` must already be that base: an `Int` for `Reps`. The constructor validates; it does not convert, so a `String` is refused before any predicate runs. Turn the value into the base first, such as with `Int.parse`, or take it as the base type. `Reps.unsafe` on an opaque type has the same rule.",
        example: "type Reps = Int where InRange(1, 100)\n\nfn parse(s: String) -> Result[Reps, ValidationError] {\n    Reps.of(s)        // ✗ `Reps.of` takes an `Int`\n}\n\nfn parse(n: Int) -> Result[Reps, ValidationError] {\n    Reps.of(n)        // ✓\n}",
        page: "/book/guides/type-system/define-and-validate",
        anchor: "validate-with-of",
    },
    Explain {
        code: "bynk.types.decimal_division_unscaled",
        blurb: "`Decimal` is exact, and a quotient often has no exact decimal answer (one third), so \
                `/` is not defined on it. `div` says what `/` leaves out: how many decimal places to \
                keep and how to round. `+`, `-` and `*` stay exact. Write \
                `a.div(b, 2, RoundingMode.HalfEven)`, with the scale and mode you need.",
        example: "fn share(total: Decimal, ways: Decimal) -> Decimal {\n    total / ways    // ✗ no exact quotient\n}\n\nfn share(total: Decimal, ways: Decimal) -> Decimal {\n    total.div(ways, 2, RoundingMode.HalfEven)    // ✓ two places, rounded\n}",
        page: "/book/reference/types",
        anchor: "decimal",
    },
    Explain {
        code: "bynk.types.derives_automatic",
        blurb: "`derives` opts a type into a capability it does not have by default, and `Ord` is the only one: it needs every field to be orderable, so you ask for it. `Eq`, `Key` and `Codable` hold for a type without being asked, so naming one does nothing, and the compiler refuses it rather than leave a clause that suggests it matters. Remove the name, which the compiler's fix does. Any other name is `bynk.generics.unknown_constraint`.",
        example: "type Point = { x: Int, y: Int } derives Eq    // ✗ `Eq` holds already\n\ntype Point = { x: Int, y: Int }               // ✓",
        page: "/book/reference/types",
        anchor: "ordering-your-own-types",
    },
    Explain {
        code: "bynk.types.derives_field_not_orderable",
        blurb: "`derives Ord` orders a type field by field, in declaration order, so every field or payload must itself have an order. A `Bool`, a `Map`, a `Set`, a function, and a named type that does not derive `Ord` itself have none. Change the field to an orderable type, such as an `Int` rank, or give the field's own type `derives Ord`, or drop `derives Ord` and sort by a key with `sortBy`.",
        example: "type Task = { title: String, done: Bool } derives Ord        // ✗ a `Bool` has no order\n\ntype Task = { title: String, priority: Int } derives Ord      // ✓",
        page: "/book/reference/types",
        anchor: "ordering-your-own-types",
    },
    Explain {
        code: "bynk.types.duplicate_example",
        blurb: "`@example` gives `Val` the one value to use for a `Matches` or `IsoDate` refinement, which it cannot invent. A second `@example` would leave two candidates and no rule for choosing, so a refinement takes at most one. Keep the one you want. An example that does not satisfy the refinement is `bynk.types.example_violates`.",
        example: "type Code = String where Matches(\"[A-Z]{3}\") @example(\"ABC\") @example(\"XYZ\")    // ✗ a second example\n\ntype Code = String where Matches(\"[A-Z]{3}\") @example(\"ABC\")                       // ✓",
        page: "/book/reference/testing",
        anchor: "val-example",
    },
    Explain {
        code: "bynk.types.duplicate_literal_arm",
        blurb: "A `match` takes the first arm whose pattern fits, so a literal that an earlier unguarded arm already has can never reach a second arm. Bynk refuses the dead arm rather than let it hide a mistake, usually a mistyped value. Change the literal or delete the arm. An or-pattern is checked alternative by alternative, so `1 | 2` followed by `2` is caught as well. The same for a variant is `bynk.types.duplicate_variant_arm`.",
        example: "fn word(n: Int) -> String {\n    match n {\n        1 => \"one\"\n        1 => \"uno\"    // ✗ `1` is already matched above\n        _ => \"other\"\n    }\n}\n\nfn word(n: Int) -> String {\n    match n {\n        1 => \"one\"\n        2 => \"two\"    // ✓\n        _ => \"other\"\n    }\n}",
        page: "/book/reference/types",
        anchor: "exhaustiveness-and-reachability",
    },
    Explain {
        code: "bynk.types.duplicate_pattern_field",
        blurb: "A record pattern lists the fields it reads, and each may be named once. Naming a field twice would bind or test the same value twice, which is almost always a slip for a different field. Keep one mention, and give it the name you want to use, as in `total: t`. The `..` at the end covers every field you leave out.",
        example: "type Order = { id: String, total: Int }\n\nfn amount(o: Order) -> Int {\n    match o {\n        Order { total, total: t, .. } => t    // ✗ `total` is named twice\n    }\n}\n\nfn amount(o: Order) -> Int {\n    match o {\n        Order { total: t, .. } => t           // ✓\n    }\n}",
        page: "/book/reference/types",
        anchor: "record-patterns",
    },
    Explain {
        code: "bynk.types.duplicate_variant_arm",
        blurb: "A `match` runs the first arm that fits, so a second unguarded arm for a variant that an earlier arm already covers can never run. Bynk refuses the dead arm rather than let it hide a slip, usually a copied arm whose pattern was never changed. Delete it, or change it to the variant you meant. Arms are compared by shape, so `Err(A)` and `Err(B)` are not duplicates. The same mistake with a literal is `bynk.types.duplicate_literal_arm`.",
        example: "type Light = | Red | Green\n\nfn go(l: Light) -> Int {\n    match l {\n        Red => 1\n        Red => 2      // ✗ `Red` is already matched above\n        Green => 3\n    }\n}\n\nfn go(l: Light) -> Int {\n    match l {\n        Red => 1\n        Green => 2    // ✓ each variant once\n    }\n}",
        page: "/book/reference/types",
        anchor: "exhaustiveness-and-reachability",
    },
    Explain {
        code: "bynk.types.embeds_ambiguous",
        blurb: "`embeds E as V` lets `?` wrap an `E` error in the variant `V`. It picks the variant by \
                the error's type, so a type may be embedded once: with two variants wrapping \
                `PayError`, `?` could not tell which one an error belongs to. Embed the type in one \
                variant, and build the other by hand where you want it.",
        example: "type OrderError =\n    | Payment(reason: PayError)\n    | Refund(reason: PayError)\n    embeds PayError as Payment, PayError as Refund    // ✗ `PayError` twice\n\ntype OrderError =\n    | Payment(reason: PayError)\n    | Refund(reason: PayError)\n    embeds PayError as Payment    // ✓ once",
        page: "/book/reference/types",
        anchor: "error-embeddings",
    },
    Explain {
        code: "bynk.types.embeds_unknown_variant",
        blurb: "`embeds E as V` lets `?` wrap an `E` error in the variant `V` of the same sum, so `V` \
                must be one of that sum's variants. Here it names none, usually a misspelling. \
                Correct the name, or add the variant it should wrap into.",
        example: "type OrderError =\n    | Payment(reason: PayError)\n    embeds PayError as Pay        // ✗ no variant `Pay`\n\ntype OrderError =\n    | Payment(reason: PayError)\n    embeds PayError as Payment    // ✓",
        page: "/book/reference/types",
        anchor: "error-embeddings",
    },
    Explain {
        code: "bynk.types.embeds_variant_shape",
        blurb: "`embeds E as V` has `?` wrap an `E` error into `V`, so `V` must take exactly that: one \
                payload field, of type `E`. A variant with a payload of another type, or with more \
                or fewer fields, has nowhere to put the error. Give `V` a single field of type `E`, \
                or embed a different variant.",
        example: "type OrderError =\n    | Payment(reason: String)\n    embeds PayError as Payment    // ✗ the payload is a `String`\n\ntype OrderError =\n    | Payment(reason: PayError)\n    embeds PayError as Payment    // ✓",
        page: "/book/reference/types",
        anchor: "error-embeddings",
    },
    Explain {
        code: "bynk.types.empty_range_pattern",
        blurb: "A range pattern `lo..hi` matches every `Int` from `lo` to `hi`, both \
                inclusive, so the low bound comes first. With `lo > hi` no value can match, \
                so the arm is dead and almost certainly a typo, and the same goes for a range \
                in a `via schema(...)` clause. Swap the bounds. A refinement \
                written `InRange(150, 0)` is the separate `bynk.types.inverted_range`.",
        example: "fn band(n: Int) -> String {\n    match n {\n        9..3 => \"mid\"    // ✗ the low bound is above the high bound\n        _ => \"other\"\n    }\n}\n\n\
                  fn band(n: Int) -> String {\n    match n {\n        3..9 => \"mid\"    // ✓\n        _ => \"other\"\n    }\n}",
        page: "/book/reference/types",
        anchor: "range-patterns",
    },
    Explain {
        code: "bynk.types.empty_refinement",
        blurb: "A refinement is a base type plus predicates, and a value must satisfy all of \
                them. When the predicates contradict each other, nothing can satisfy them: \
                the type has no values, and every `.of` and every boundary check would fail. \
                The compiler works out the combined range of `Positive`, `NonNegative` and \
                `InRange`, and the combined length of the length predicates, and refuses one \
                that is empty. Drop or correct the predicate that conflicts, and read the \
                note, which gives the empty range it found.",
        example: "type Debt = Int where Positive && InRange(-10, -1)    // ✗ positive, yet at most -1\n\n\
                  type Debt = Int where Positive && InRange(1, 10)      // ✓",
        page: "/book/reference/refined-types",
        anchor: "predicates",
    },
    Explain {
        code: "bynk.types.err_value_mismatch",
        blurb: "`Err(e)` takes its error type from the surrounding `Result[T, E]`, the annotation or the function's return type, so `Err(404)` where `E` is `String` is refused at the value, with no conversion. Pass an `E`, or change the declared error type. The success half is `bynk.types.ok_value_mismatch`; with nothing to fix the `Result`, it is `bynk.types.cannot_infer_result_type_params`.",
        example: "fn total() -> Result[Int, String] {\n    Err(404)    // ✗ an `Int`, where the error type is `String`\n}\n\nfn total() -> Result[Int, String] {\n    Err(\"not found\")    // ✓\n}",
        page: "/book/guides/type-system/result-and-optionals",
        anchor: "construct-values",
    },
    Explain {
        code: "bynk.types.example_not_literal",
        blurb: "`@example` takes exactly one literal of the refinement's base type. It is read where \
                the type is declared, not evaluated, so a call such as `Code.of(\"ABC\")`, a name or \
                a literal of another type is refused. Write the value itself, as a string for a \
                `String` base.",
        example: "type Code = String where Matches(\"[A-Z]{3}\") @example(Code.of(\"ABC\"))    // ✗ a call, not a literal\n\ntype Code = String where Matches(\"[A-Z]{3}\") @example(\"ABC\")            // ✓\n\nconst Sample: String = \"ABC\"\ntype Tag = String where Matches(\"[A-Z]{3}\") @example(Sample)   // ✓ a `String` constant bound to a literal",
        page: "/book/reference/testing",
        anchor: "val-example",
    },
    Explain {
        code: "bynk.types.example_on_named_field",
        blurb: "On a record field, `@example` pins the field's own value, which exists only where \
                the field has a base type. A field of a named type takes its value from that type, \
                so an example on the field would have no effect and is refused. Put the \
                `@example` on the named type's declaration, where `Val` reads it.",
        example: "type Code = String where Matches(\"[A-Z]{3}\")\ntype Ticket = { code: Code where Matches(\"[A-Z]+\") @example(\"ABC\") }    // ✗ `Code` is a named type\n\ntype Code = String where Matches(\"[A-Z]{3}\") @example(\"ABC\")    // ✓ pin it on the type\ntype Ticket = { code: Code }",
        page: "/book/reference/testing",
        anchor: "val-example",
    },
    Explain {
        code: "bynk.types.example_unneeded",
        blurb: "`Val` cannot invent a string that matches a pattern, so a `Matches` or `IsoDate` \
                refinement takes an `@example` for it to use. Every other refinement already has a \
                value `Val` makes, so there is nothing for the example to do and it is refused. \
                Remove the annotation.",
        example: "type Sku = String where NonEmpty @example(\"a\")    // ✗ `Val` already makes one\n\ntype Sku = String where NonEmpty                  // ✓",
        page: "/book/reference/testing",
        anchor: "val-example",
    },
    Explain {
        code: "bynk.types.example_violates",
        blurb: "`@example` is the value `Val` uses for a type it cannot fabricate, so it must be a \
                valid one. The compiler checks it where the type is declared, against every \
                predicate of the refinement, whether or not a test uses it; an example that fails \
                one would give tests a value the type refuses. Write an example that satisfies the \
                whole refinement.",
        example: "type Code = String where Matches(\"[A-Z]{3}\") @example(\"abc\")    // ✗ lower case\n\ntype Code = String where Matches(\"[A-Z]{3}\") @example(\"ABC\")    // ✓",
        page: "/book/reference/testing",
        anchor: "val-example",
    },
    Explain {
        code: "bynk.types.extensible_match_needs_wildcard",
        blurb: "Some standard-library enums, among them `FetchError` and `Method`, are \
                extensible: a minor release may add a variant. A `match` that names every \
                variant they have today would then stop covering them, so the compiler asks \
                for a wildcard arm now, and the `match` keeps compiling when a variant \
                arrives. Listing every current variant is not enough. Add `_`, or `Err(_)` \
                where the enum is wrapped, and put there whatever should happen for a \
                variant you did not name. An enum of your own with the same name is not \
                extensible; the rule follows the first-party enum. For an ordinary enum, a \
                missing arm is `bynk.types.non_exhaustive_match`.",
        example: "fn verb(m: Method) -> String {\n    match m {           // ✗ every variant, but no wildcard\n        Get => \"read\"\n        Post => \"write\"\n        Put => \"write\"\n        Delete => \"write\"\n        Patch => \"write\"\n        Head => \"read\"\n        Options => \"read\"\n    }\n}\n\n\
                  fn verb(m: Method) -> String {\n    match m {\n        Get => \"read\"\n        _ => \"write\"       // ✓ also covers variants added later\n    }\n}",
        page: "/book/reference/bynk-capabilities",
        anchor: "extensible-enums",
    },
    Explain {
        code: "bynk.types.field_access_on_non_record",
        blurb: "`.name` reads a field, and only a record has fields. A `String`, an `Int` or a `List` has methods instead, called with parentheses, so `name.length` is not a field read but `name.length()` is a call. A refined type is already its base type, so there is nothing to unwrap. To get at a sum type's payload, use `match`. A field a record lacks is `bynk.types.unknown_field`.",
        example: "fn size(name: String) -> Int {\n    name.length    // ✗ `String` is not a record; `length` is a method\n}\n\nfn size(name: String) -> Int {\n    name.length()    // ✓\n}",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.types.field_annotation_unknown",
        blurb: "`@name(\"…\")` is the only annotation a record field takes: it gives the field the key it has on an HTTP request where that key is not an identifier. Any other annotation is refused rather than ignored, so a misspelt one cannot silently do nothing. Write `@name`, or drop the annotation.",
        example: "type Filter = {\n    @rename(\"page-size\") pageSize: Option[Int],    // ✗ only `@name` exists\n}\n\ntype Filter = {\n    @name(\"page-size\") pageSize: Option[Int],      // ✓\n}",
        page: "/book/reference/types",
        anchor: "field-name",
    },
    Explain {
        code: "bynk.types.field_default_on_variant",
        blurb: "Only a record's fields take defaults. A variant is built like a call, with every \
                payload field given, so a default on a payload field would be an optional argument \
                by another name. Remove it and give the field wherever the variant is built, or \
                hold the optional values in a record, which can have defaults, and make that \
                record the payload.",
        example: "type Retry =\n    | Never\n    | Times(n: Int = 3)    // ✗ a variant's field has no default\n\ntype Backoff = { n: Int = 3 }\n\ntype Retry =\n    | Never\n    | Times(opts: Backoff)    // ✓ the record carries the default",
        page: "/book/reference/types",
        anchor: "field-defaults",
    },
    Explain {
        code: "bynk.types.field_name_bad_arg",
        blurb: "`@name` renames a field's key on an HTTP request, and the compiler reads that key at compile time, so it takes exactly one string literal, or a `String` constant bound to one: not a number, a computed value, a second argument or none. A constant must be declared exactly `String` (not a refined type) and bound to a string literal or to another such constant. Write the key as a literal, as in `@name(\"__Host-sid\")`, or name it once with a constant.",
        example: "type Filter = {\n    @name(1) pageSize: Option[Int],             // ✗ not a string literal\n}\n\nconst PageKey: String = \"page-size\"\n\ntype Filter = {\n    @name(PageKey) pageSize: Option[Int],       // ✓ a `String` constant bound to a literal\n}\n\ntype Filter = {\n    @name(\"page-size\") pageSize: Option[Int],   // ✓\n}",
        page: "/book/reference/types",
        anchor: "field-name",
    },
    Explain {
        code: "bynk.types.field_name_case_conflict",
        blurb: "A `cookies` or `query` record is read by name, and names that differ only in case are easily confused: a client that sends `page` would not match `Page`, yet both look like the same parameter. So two fields of such a record may not have wire names that differ only in case. Rename one. Two fields with exactly the same wire name are `bynk.types.field_name_duplicate`.",
        example: "type Filter = {\n    @name(\"Page\") a: Option[Int],\n    @name(\"page\") b: Option[Int],    // ✗ differs from `Page` only in case\n}\n\ntype Filter = {\n    @name(\"Page\") a: Option[Int],\n    @name(\"size\") b: Option[Int],    // ✓\n}",
        page: "/book/reference/types",
        anchor: "field-name",
    },
    Explain {
        code: "bynk.types.field_name_duplicate",
        blurb: "A field's wire name is the key a request reader looks it up by, so two fields of one record cannot share one: both would read the same value. A field's wire name is its `@name`, or its own name when it has none. A field may also carry only one `@name`. Give each field its own key. Names differing only in case are `bynk.types.field_name_case_conflict`.",
        example: "type Filter = {\n    @name(\"page\") a: Option[Int],\n    page: Option[Int],       // ✗ reads `page` again\n}\n\ntype Filter = {\n    @name(\"page\") a: Option[Int],\n    size: Option[Int],       // ✓\n}",
        page: "/book/reference/types",
        anchor: "field-name",
    },
    Explain {
        code: "bynk.types.field_name_invalid",
        blurb: "A `@name` has to be usable as a key where the record is read. It may not be empty, and on a `cookies` record it must be a cookie name, an RFC 9110 token: no spaces, quotes or separators such as `;` `,` or `=`. A name the request could never carry would make the field unreachable. Use a valid token, such as `__Host-sid`.",
        example: "type Session = {\n    @name(\"session id\") sid: Option[String],    // ✗ a space is not allowed in a cookie name\n}\n\ntype Session = {\n    @name(\"session-id\") sid: Option[String],    // ✓\n}",
        page: "/book/reference/types",
        anchor: "field-name",
    },
    Explain {
        code: "bynk.types.field_name_not_request_record",
        blurb: "`@name` renames the key a request reader looks for. Only a record that is the `cookies`, `query` or `@form`/`@multipart` `body` parameter of an `on http` handler is read that way, so on any other record the annotation would do nothing. Remove it, or make the record that parameter. A record used as a JSON body or result may not carry `@name` either: `bynk.types.field_name_record_not_boundary`.",
        example: "type Plain = {\n    @name(\"x-y\") a: String,    // ✗ `Plain` is not read from a request\n}\n\ntype Plain = {\n    a: String,                // ✓\n}",
        page: "/book/reference/types",
        anchor: "field-name",
    },
    Explain {
        code: "bynk.types.field_name_record_not_boundary",
        blurb: "A record with `@name` fields is read off a request by those names, but its JSON form uses the field names, so using it as a JSON body, a result, a store field, an event or a call argument would make the two forms disagree. It may only be a `cookies`, `query` or `@form`/`@multipart` `body` parameter. Use a second record, without `@name`, for the other position.",
        example: "type Filter = {\n    @name(\"page-size\") pageSize: Option[Int],\n}\n\ntype FilterView = {\n    pageSize: Option[Int],\n}\n\non GET(\"/f\") (query: Filter) -> Effect[HttpResult[Filter]] by v: Visitor { … }        // ✗\n\non GET(\"/f\") (query: Filter) -> Effect[HttpResult[FilterView]] by v: Visitor { … }    // ✓",
        page: "/book/reference/types",
        anchor: "field-name",
    },
    Explain {
        code: "bynk.types.field_refinement_not_base",
        blurb: "An inline refinement, `price: Money where Positive`, applies predicates to a value of a base type, or of a refined type over one. A record or a sum has no such value for a predicate to test. Put the refinement on the field that holds the number, inside the record, or declare a refined type and use its name.",
        example: "type Money = { cents: Int }\ntype Line = { price: Money where Positive }    // ✗ `Money` is a record, which no predicate fits\n\ntype Money = { cents: Int where Positive }     // ✓ refine the field that holds the number\ntype Line = { price: Money }",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.types.field_value_mismatch",
        blurb: "A record literal checks each field against the type the declaration gives it, with no implicit conversion, so `y: \"0\"` for an `Int` field is refused where it is written. Give the field a value of its declared type, or convert it first. A field the type lacks is `bynk.resolve.unknown_field`, and one left out is `bynk.resolve.missing_field`.",
        example: "type Point = { x: Int, y: Int }\n\nfn origin() -> Point {\n    Point { x: 0, y: \"0\" }    // ✗ `y` is an `Int`\n}\n\nfn origin() -> Point {\n    Point { x: 0, y: 0 }    // ✓\n}",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.types.file_at_boundary",
        blurb: "A `File` is one uploaded part of a `@multipart` request, held in memory for the handler that receives it. A record may have a `File` field, alone, in an `Option` or in a `List`, which is how a `@multipart` `body` names its parts. But a `File`, or a record carrying one, is refused as an agent store field or key, an event field, a handler result, or any handler parameter but that `body`. Take the part's `bytes`, and the `filename` and `contentType` you need, in the handler, and keep those.",
        example: "consumes bynk { File }\n\nagent Gallery {\n    key id: String\n    store photo: Cell[Option[File]]     // ✗ an upload is not kept as a `File`\n}\n\nagent Gallery {\n    key id: String\n    store photo: Cell[Option[Bytes]]    // ✓ keep its bytes\n}",
        page: "/book/reference/types",
        anchor: "file",
    },
    Explain {
        code: "bynk.types.form_uncodable",
        blurb: "`Form.decode[T]` and `Form.encode` read and write the flat `key=value` text a `@form` route handles, so `T` must be a record of fields a form can carry: a `String`, `Int`, `Bool`, `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` or a `List`. Form keys have no nesting, so a nested record, or a type that is not a record, is refused. Put the nested record's fields in `T` itself. A `@form` route's own body has the same rule: `bynk.http.form_field_not_stringy`.",
        example: "type Address = { city: String }\ntype Signup = { email: String, address: Address }\n\nForm.decode[Signup](text)    // ✗ `address` is a nested record\n\ntype Signup = { email: String, city: String }\n\nForm.decode[Signup](text)    // ✓",
        page: "/book/reference/types",
        anchor: "form-codec",
    },
    Explain {
        code: "bynk.types.function_at_boundary",
        blurb: "A function type can be written only where a value stays in the running \
                program: as a `fn` or lambda parameter, a return, or a local. A value that is \
                serialised, stored or sent has to be data, and a function has no wire form, so \
                a function type in a record field, a sum payload, an event field, a service or \
                agent handler signature, an agent key or store field, a capability operation \
                or an actor identity is refused where it is written. Keep the function out of \
                the type: pass it as a parameter to the `fn` that needs it, or store the data \
                it is built from, such as a name or a number, and rebuild the behaviour on \
                the other side.",
        example: "type Job = {\n  name: String,\n  run: Int -> Int,    // ✗ a record field cannot hold a function\n}\n\n\
                  type Job = {\n  name: String,\n  step: Int,\n}\n\nfn run(j: Job, f: Int -> Int) -> Int {    // ✓ a parameter can\n    f(j.step)\n}",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.types.guard_not_bool",
        blurb: "A guard, as in `pattern if condition => …`, decides whether an arm applies, so it must be a `Bool`: a number or an `Option` is not read as true or false. Say what you mean with a comparison, such as `r > 10`. A guarded arm covers nothing for exhaustiveness, because its guard may fail. The condition of an `if` expression is `bynk.types.if_non_bool_cond`.",
        example: "type Shape =\n  | Circle(radius: Int)\n  | Dot\n\nfn label(s: Shape) -> String {\n    match s {\n        Circle(r) if r => \"round\"    // ✗ `r` is an `Int`\n        Circle(_) => \"small\"\n        Dot => \"dot\"\n    }\n}\n\nfn label(s: Shape) -> String {\n    match s {\n        Circle(r) if r > 10 => \"round\"    // ✓\n        Circle(_) => \"small\"\n        Dot => \"dot\"\n    }\n}",
        page: "/book/reference/types",
        anchor: "matching",
    },
    Explain {
        code: "bynk.types.headers_at_boundary",
        blurb: "`RequestHeaders` is the view of one request's headers, given to the handler that is answering it. It is read there and dropped: headers can carry credentials such as `Authorization`, and a copy kept or sent on would outlive the request. It is accepted only as an HTTP handler's `headers` parameter, and refused as a record, sum or event field, an agent key or store field, a capability operation, or any other handler parameter or result. Read the header you need (`headers.get(\"…\")`) and keep that `String`. See also `bynk.types.request_query_at_boundary`.",
        example: "type Seen = {\n    headers: RequestHeaders,    // ✗ a request's headers are not data to keep\n}\n\ntype Seen = {\n    userAgent: String,          // ✓ keep the value you read\n}",
        page: "/book/reference/http",
        anchor: "request-headers",
    },
    Explain {
        code: "bynk.types.held_at_boundary",
        blurb: "A `Connection` is a held resource, built and disposed of in place, so it is never \
                stored or sent: it cannot be a record, sum or event field, or part of a service or \
                agent handler signature. An agent may keep one in its own state, as \
                `Cell[Option[Connection[F]]]` or `Map[K, Connection[F]]`. Keep it there, and pass \
                what you need from it, not the connection.",
        example: "type Session = {\n    conn: Connection[Frame],    // ✗ a held resource is not a field\n}\n\nagent Room {\n    key id: String\n\n    store conns: Map[String, Connection[Frame]]    // ✓ held in agent state\n}",
        page: "/book/reference/types",
        anchor: "connection",
    },
    Explain {
        code: "bynk.types.held_not_comparable",
        blurb: "A held `Connection` is a live resource with an identity: two values are the same connection or they are not, so there is no value to compare. So `==` on one, or on a type that holds one, is refused. Compare an id that you keep next to the connection. A `Stream` has `bynk.types.stream_not_comparable`, and functions and effects have `bynk.types.not_comparable`.",
        example: "type Frame = { text: String }\n\nfn same(a: Connection[Frame], b: Connection[Frame]) -> Bool {\n    a == b                            // ✗ a connection has identity, not value\n}\n\nfn same(a: String, b: String) -> Bool {    // ✓ compare ids kept beside them\n    a == b\n}",
        page: "/book/reference/types",
        anchor: "connection",
    },
    Explain {
        code: "bynk.types.if_branch_mismatch",
        blurb: "Both branches of an `if` can be its value, so they must have one type: their common supertype, where a refined type and its base agree at the base. With a `String` in one branch and an `Int` in the other, the caller has no single type to rely on. Make the branches agree, converting where needed. A `match` whose arms disagree is `bynk.types.match_arm_mismatch`.",
        example: "fn grade(score: Int) -> String {\n    if score >= 50 { \"pass\" } else { 0 }    // ✗ a `String` branch and an `Int` branch\n}\n\nfn grade(score: Int) -> String {\n    if score >= 50 { \"pass\" } else { \"fail\" }    // ✓\n}",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.types.if_non_bool_cond",
        blurb: "An `if` chooses a branch on a `Bool`, and Bynk has no truthiness: a number, a string, an `Option` or a list is never read as true or false. Say what you mean with a comparison (`count > 0`), or test an `Option` with `is Some(…)`. A match-arm guard has the same rule, as `bynk.types.guard_not_bool`.",
        example: "fn describe(count: Int) -> String {\n    if count { \"some\" } else { \"none\" }    // ✗ an `Int`, not a `Bool`\n}\n\nfn describe(count: Int) -> String {\n    if count > 0 { \"some\" } else { \"none\" }    // ✓\n}",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.types.if_without_else_requires_unit",
        blurb: "An `if` with no `else` has nothing to produce when its condition is false, so Bynk gives that path the value `()`. The `then` branch must therefore be `()` too, or `Effect[()]` for a branch that only performs effects; a branch that yields a `String` would leave the other path without one. Add an `else` that produces a value of the same type, or make the branch a unit effect.",
        example: "fn sign(n: Int) -> String {\n    if n > 0 { \"positive\" }    // ✗ no `else`, so the value would be `()`\n}\n\nfn sign(n: Int) -> String {\n    if n > 0 { \"positive\" } else { \"not positive\" }    // ✓\n}",
        page: "/book/spec/static-semantics",
        anchor: "",
    },
    Explain {
        code: "bynk.types.interpolation_non_scalar",
        blurb: "A `\\(…)` hole in a string takes a `String`, `Int`, `Float`, `Bool`, \
                `Decimal`, or a refined type over one of them, and nothing else. A record or \
                sum has no canonical text, and Bynk will not print an unreadable stand-in \
                such as `[object Object]`. An opaque type hides its base, so use `.raw` \
                first. Turn other values into a `String` yourself: `toHex()` or `toBase64()` \
                on `Bytes`, `toString()` on a `Duration`, `toIso()` or `toString()` on an \
                `Instant`, or interpolate the fields of a record, a `ValidationError` or a \
                `DecodeError`.",
        example: "type User = { name: String }\n\n\
                  fn label(u: User) -> String {\n    \"user: \\(u)\"         // ✗ a record has no string form\n}\n\n\
                  fn label(u: User) -> String {\n    \"user: \\(u.name)\"    // ✓ interpolate the field\n}",
        page: "/book/reference/grammar",
        anchor: "rule-string_interpolation",
    },
    Explain {
        code: "bynk.types.invalid_regex",
        blurb: "A `Matches` pattern, and the pattern of a regex method such as `containsRegex`, \
                must be a valid regular expression. The emitted check runs it under \
                JavaScript's `RegExp`, so the compiler validates it with those rules: a \
                pattern the Rust `regex` crate accepts, such as `(?P<name>…)`, may be refused \
                here, and the compiler would otherwise ship a check that throws at run time. \
                A regex method's pattern may open with one inline-flag group of `i`, `m` \
                and `s`, each at most once. Fix the syntax; the note carries the parser's \
                message. A valid but dangerous pattern is `bynk.types.catastrophic_regex`.",
        example: "type Code = String where Matches(\"[A-Z\")     // ✗ the class is never closed\n\n\
                  type Code = String where Matches(\"[A-Z]+\")   // ✓",
        page: "/book/reference/types",
        anchor: "regular-expressions",
    },
    Explain {
        code: "bynk.types.inverted_range",
        blurb: "`InRange(lo, hi)` is inclusive at both ends, so it needs `lo <= hi`. With the \
                bounds the wrong way round the refinement admits no value, which is almost \
                certainly a slip, so the compiler refuses it. Swap them; the diagnostic \
                offers that fix. On an `Int` or `Float` the empty range is also reported as \
                `bynk.types.empty_refinement`, which is what contradictory predicates give \
                too. An inverted range in a `match` arm is `bynk.types.empty_range_pattern`.",
        example: "type Price = Decimal where InRange(100.00d, 0.00d)    // ✗ the minimum is above the maximum\n\n\
                  type Price = Decimal where InRange(0.00d, 100.00d)    // ✓",
        page: "/book/reference/refined-types",
        anchor: "predicates",
    },
    Explain {
        code: "bynk.types.is_base_mismatch",
        blurb: "`value is Quantity` runs a refined type's predicates against the value. A refined type is a base type plus predicates, so the value must have that base: an `Int` can be checked against `Int where …`, but a `String` has nothing for an `Int` range to test. Check the value against a refined type over its own type, or convert it first, for example by parsing the `String`.",
        example: "type Quantity = Int where InRange(1, 100)\n\ns is Quantity    // ✗ `s` is a `String`, `Quantity` is over `Int`\n\nn is Quantity    // ✓ `n` is an `Int`",
        page: "/book/reference/refined-types",
        anchor: "narrowing-with-is",
    },
    Explain {
        code: "bynk.types.is_literal_pattern",
        blurb: "`is` tests a value's variant or refined type, not its value, so `x is 31` would read as a type test while meaning equality. Bynk refuses it rather than let the two readings blur. Compare with `==` instead. In a `match`, a literal is a fine pattern, because a `match` arm tests a value there.",
        example: "fn thirty(x: Int) -> Bool {\n    x is 31           // ✗ `is` tests a type, not a value\n}\n\nfn thirty(x: Int) -> Bool {\n    x == 31           // ✓\n}",
        page: "/book/guides/type-system/narrow-with-is",
        anchor: "",
    },
    Explain {
        code: "bynk.types.is_non_sum",
        blurb: "`value is Variant` asks which variant a sum, `Result` or `Option` holds, so its left side must be one of those. On an `Int`, a `String` or a record there is no variant to ask about. To test a number or a string against a refined type, name that type: `n is Quantity`. To compare by value, use `==`. A variant the sum does not have is `bynk.types.is_unknown_variant`.",
        example: "type Light = | Red | Green\n\nfn ready(n: Int) -> Bool {\n    n is Green        // ✗ `n` is an `Int`, not a `Light`\n}\n\nfn ready(l: Light) -> Bool {\n    l is Green        // ✓\n}",
        page: "/book/guides/type-system/narrow-with-is",
        anchor: "test-a-variant",
    },
    Explain {
        code: "bynk.types.is_range_pattern",
        blurb: "`is` tests a value's variant or refined type, not whether a number lies in a range, so `age is 13..19` would read as a type test while comparing values. Bynk refuses it. Compare the bounds, write a `match` arm `13..19`, or declare a refined type with `InRange(13, 19)` and test against that. The literal case is `bynk.types.is_literal_pattern`.",
        example: "fn teen(age: Int) -> Bool {\n    age is 13..19                // ✗ `is` tests a type, not a range\n}\n\nfn teen(age: Int) -> Bool {\n    age >= 13 && age <= 19       // ✓\n}",
        page: "/book/guides/type-system/narrow-with-is",
        anchor: "",
    },
    Explain {
        code: "bynk.types.is_refined_pattern",
        blurb: "`is` tests a value against a type or a variant. An inline refinement, `_ where Positive`, is a pattern for a `match` arm, and is not accepted after `is`: a rule worth testing is worth a name, which also lets every other place use it. Declare a refined type and write `n is Natural`, or move the test into a `match` arm.",
        example: "n is _ where Positive    // ✗ not a pattern for `is`\n\ntype Natural = Int where Positive\n\nn is Natural             // ✓ name the refinement",
        page: "/book/reference/refined-types",
        anchor: "narrowing-with-is",
    },
    Explain {
        code: "bynk.types.is_unknown_variant",
        blurb: "`l is Amber` asks whether a `Light` is its `Amber` variant, which only means something if `Light` has one. A name that is not a variant of the sum is usually a typo or a variant left over from an earlier version of the type. Use one the type declares. On a value that is not a sum at all, the error is `bynk.types.is_non_sum`.",
        example: "type Light = | Red | Green\n\nfn ready(l: Light) -> Bool {\n    l is Amber        // ✗ `Light` has no variant `Amber`\n}\n\nfn ready(l: Light) -> Bool {\n    l is Green        // ✓\n}",
        page: "/book/guides/type-system/narrow-with-is",
        anchor: "test-a-variant",
    },
    Explain {
        code: "bynk.types.json_uncodable",
        blurb: "`Json.encode` and `Json.decode` work on values that have a wire form, and a \
                function does not: a value that can run has no JSON text to stand for it. \
                The codec covers the base types, named types and the built-in containers \
                over them. It does not cover functions, effects, streams or a type that \
                carries one, `bynk.crypto` keys or the error builtins. Encode the data the \
                function produces, or the record that describes it, not the function itself.",
        example: "fn dump(f: Int -> Int) -> String {\n    Json.encode(f)       // ✗ a function has no JSON form\n}\n\n\
                  fn dump(f: Int -> Int) -> String {\n    Json.encode(f(1))    // ✓ encode a value it produces\n}",
        page: "/book/reference/types",
        anchor: "the-json-codec",
    },
    Explain {
        code: "bynk.types.key_at_boundary",
        blurb: "A `bynk.crypto` `SigningKey` or `VerifyKey` is a handle to a key the platform \
                holds non-extractable, and it lives only for the call that imported it. \
                Storing, serialising, logging or sending it would either leak the secret or \
                mean nothing on the other side, so one cannot be a `store` field, a record or \
                event field, or a handler parameter or result. Keep the key's bytes, such as \
                the PKCS#8 `Bytes`, in `Secrets` or a field, and import the key again with \
                `Crypto.importSigningKey` where you sign. A capability operation may take one.",
        example: "type Credentials = {\n    owner: String,\n    key: SigningKey,    // ✗ a key cannot be held in a type\n}\n\n\
                  type Credentials = {\n    owner: String,\n    pkcs8: Bytes,       // ✓ keep the bytes, import the key where it signs\n}",
        page: "/book/reference/bynk-capabilities",
        anchor: "",
    },
    Explain {
        code: "bynk.types.key_not_orderable",
        blurb: "`sortBy`, `sortByDescending`, `min` and `max` order a list, or a storage \
                `Query`, by the value the key function returns, so that value needs a \
                total order. A record or sum is not ordered by accident, because opting in \
                makes field and variant order significant. Return a field that is already \
                orderable, or add `derives Ord` to the type. The operators `<` and `>` on an \
                unordered type are `bynk.types.not_orderable`.",
        example: "type Item = { id: Int }\n\n\
                  fn byId(items: List[Item]) -> List[Item] {\n    items.sortBy((i) => i)       // ✗ `Item` has no order\n}\n\n\
                  fn byId(items: List[Item]) -> List[Item] {\n    items.sortBy((i) => i.id)    // ✓ `Int` does\n}",
        page: "/book/reference/types",
        anchor: "ordering-your-own-types",
    },
    Explain {
        code: "bynk.types.lambda_mismatch",
        blurb: "A lambda is checked against the function type expected where it is written, \
                such as the parameter it is passed for. It must take the same number of \
                parameters, any annotation on a parameter must match that parameter's type, \
                and its body must produce the result type, because the function it is passed \
                to calls it with that shape and uses what comes back. Change the lambda to \
                fit, or change the function type if that is what is wrong.",
        example: "fn apply(f: Int -> Int, x: Int) -> Int {\n    f(x)\n}\n\n\
                  fn total() -> Int {\n    apply((a, b) => a + b, 5)    // ✗ `f` takes one argument\n}\n\n\
                  fn total() -> Int {\n    apply((a) => a + 1, 5)       // ✓\n}",
        page: "/book/reference/grammar",
        anchor: "rule-lambda_expr",
    },
    Explain {
        code: "bynk.types.let_annotation_mismatch",
        blurb: "A `let` with a type annotation declares what the binding is, and its value \
                must have exactly that type. The annotation is the contract that later code \
                reads, so a value of another type is refused at the `let`, not where it is \
                later used. Nothing is converted for you: change the value, convert it \
                explicitly, or correct the annotation if the value is what you meant. The \
                same rule governs a function's declared return, which is \
                `bynk.types.return_mismatch`.",
        example: "fn run() -> Int {\n    let n: Int = \"three\"    // ✗ a `String`, annotated `Int`\n    n\n}\n\n\
                  fn run() -> Int {\n    let n: Int = 3          // ✓\n    n\n}",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.types.list_element_mismatch",
        blurb: "A list has one element type, taken from the context if there is one and otherwise from the first element, and every element must have it. `[1, 2, \"3\"]` mixes `Int` and `String`, and the error is on the element that disagrees. Make the elements one type, or wrap them in a sum type if the list really holds more than one kind of value. An empty `[]` with no context is `bynk.types.uninferable_element_type`.",
        example: "fn scores() -> List[Int] {\n    [1, 2, \"3\"]    // ✗ a `String` in a `List[Int]`\n}\n\nfn scores() -> List[Int] {\n    [1, 2, 3]    // ✓\n}",
        page: "/book/reference/types",
        anchor: "built-in-generic-types",
    },
    Explain {
        code: "bynk.types.list_pattern_on_non_list",
        blurb: "A list pattern such as `[x, ..]` matches a value by its length and its elements, \
                so the value must be a `List[T]`. A `Set`, a `Map`, an `Int` or a record has no \
                such shape, and a record or sum is matched by its own pattern. Convert the \
                value first, as `s.toList()` does for a `Set`, or match on what the value \
                actually is. The same rule applies to a list pattern after `is` and in \
                `let … else`.",
        example: "fn first(s: Set[Int]) -> Int {\n    match s {\n        [x, ..] => x    // ✗ a `Set` is not a `List`\n        _ => 0\n    }\n}\n\n\
                  fn first(s: List[Int]) -> Int {\n    match s {\n        [x, ..] => x    // ✓\n        _ => 0\n    }\n}",
        page: "/book/reference/types",
        anchor: "list-patterns",
    },
    Explain {
        code: "bynk.types.match_arm_mismatch",
        blurb: "A `match` is an expression, so its arms must produce one type: the least upper \
                bound of their types, where a refined type and its base agree at the base. If \
                one arm gives a `String` and another an `Int`, there is no single type for the \
                caller to rely on. Make the arms agree, converting where needed. The rule also \
                holds for `let … else`, whose `else` must produce the type of the rest of its \
                block.",
        example: "type Status = enum { Pending, Active, Done }\n\nfn label(s: Status) -> String {\n    match s {\n        Pending => \"waiting\"\n        Active => 1           // ✗ an `Int`, where the others are `String`\n        Done => \"finished\"\n    }\n}\n\n\
                  fn label(s: Status) -> String {\n    match s {\n        Pending => \"waiting\"\n        Active => \"running\"   // ✓\n        Done => \"finished\"\n    }\n}",
        page: "/book/reference/types",
        anchor: "matching",
    },
    Explain {
        code: "bynk.types.match_non_sum_discriminant",
        blurb: "`match` chooses between alternatives, so it needs a value that has some: a sum, an enum, a `Result` or `Option`, an `Int`, `String` or `Bool`, or a list matched by list patterns. A `Float` or a function has none, and a record has a single shape, so it can be matched only with a record pattern such as `Point { x, .. }`. Read a field directly, and use `if` to branch on a condition.",
        example: "type Point = { x: Int, y: Int }\n\nfn first(p: Point) -> Int {\n    match p {         // ✗ a record has one shape, not variants\n        _ => 1\n    }\n}\n\nfn first(p: Point) -> Int {\n    p.x               // ✓ read the field\n}",
        page: "/book/reference/types",
        anchor: "matching",
    },
    Explain {
        code: "bynk.types.method_arity",
        blurb: "A method was called with a different number of arguments from the number it \
                declares. The count excludes `self`, which the receiver supplies, so \
                `fn Box.adjust(self, delta: Int)` takes one argument. Pass exactly that \
                many. The same code covers the built-in methods of `String`, `List`, `Map` and the other \
                kernel types.",
        example: "type Box = { value: Int }\n\n\
                  fn Box.adjust(self, delta: Int) -> Box {\n    Box { value: self.value + delta }\n}\n\n\
                  fn bump(b: Box) -> Box {\n    b.adjust()     // ✗ expects 1 argument\n}\n\n\
                  fn bump(b: Box) -> Box {\n    b.adjust(1)    // ✓\n}",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.types.method_not_found",
        blurb: "A method call names an instance method that the receiver's type does not \
                have. An instance method is a function declared with `self` as its first \
                parameter. A function declared on a type without `self` is static, and is \
                called on the type, as `Counter.zero()`, never on a value. This also fires on \
                a typo, and on a built-in type that has methods but not this one, since only the \
                methods the type documents exist.",
        example: "type Counter = { count: Int }\n\n\
                  fn Counter.zero() -> Counter {\n    Counter { count: 0 }\n}\n\n\
                  fn reset(c: Counter) -> Counter {\n    c.zero()          // ✗ `zero` has no `self`: not an instance method\n}\n\n\
                  fn reset(c: Counter) -> Counter {\n    Counter.zero()    // ✓ a static function is called on the type\n}",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.types.method_on_non_named_type",
        blurb: "A method call needs a type that has methods. A user-declared type does, and so do the built-ins that document them, such as `String` and `List`, but a function value, a `Bool` or `()` has none, so `f.apply(n)` has nothing to look up. A function is called directly, as `f(n)`. For a type that does have methods, a name it lacks is `bynk.types.method_not_found`.",
        example: "fn twice(f: (Int) -> Int, n: Int) -> Int {\n    f.apply(f.apply(n))    // ✗ a function value has no methods\n}\n\nfn twice(f: (Int) -> Int, n: Int) -> Int {\n    f(f(n))    // ✓ call it directly\n}",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.types.mixed_pattern_bindings",
        blurb: "A variant pattern binds its payload either by field name, `Circle(r: x, d: y)`, or by position, `Circle(x, y)`. Mixing the two leaves it unclear which field a bare name takes, so Bynk wants one style in each pattern. Name every field, or none.",
        example: "type Shape = | Circle(r: Int, d: Int) | Dot\n\nfn size(s: Shape) -> Int {\n    match s {\n        Circle(r: x, y) => x    // ✗ one named binding, one positional\n        Dot => 0\n    }\n}\n\nfn size(s: Shape) -> Int {\n    match s {\n        Circle(x, y) => x       // ✓ all positional\n        Dot => 0\n    }\n}",
        page: "/book/reference/types",
        anchor: "matching",
    },
    Explain {
        code: "bynk.types.negative_length",
        blurb: "`MinLength`, `MaxLength` and `Length` count the characters of a string, and a length cannot be below zero, so `MinLength(-1)` would hold for every string and `MaxLength(-1)` for none. The compiler takes a negative argument to be a mistake and refuses it. Give a length of zero or more. A set of predicates that no value can satisfy is `bynk.types.empty_refinement`.",
        example: "type Short = String where MinLength(-1)    // ✗ a length is never negative\n\ntype Short = String where MinLength(8)     // ✓",
        page: "/book/reference/refined-types",
        anchor: "string",
    },
    Explain {
        code: "bynk.types.never_inferred",
        blurb: "A value that only faults, such as `unreachable(\"…\")` or `todo(\"…\")`, has the \
                bottom type `never returns`, which stands in for any type. If an unannotated \
                `let` or `<-` binding took it, every later use would type-check whatever it \
                was used as, and the type cannot be written in source to say otherwise. Annotate \
                the binding with the type it is meant to have. A lambda that only faults is \
                annotated with a function type.",
        example: "fn later() -> Int {\n    let x = unreachable(\"never\")    // ✗ `x` would have no usable type\n    x\n}\n\n\
                  fn later() -> Int {\n    let x: Int = unreachable(\"never\")    // ✓\n    x\n}",
        page: "/book/reference/types",
        anchor: "faults",
    },
    Explain {
        code: "bynk.types.no_numeric_coercion",
        blurb: "`Int`, `Float` and `Decimal` are never mixed implicitly, in an operator, \
                a comparison or an `InRange` bound. Each conversion can lose or invent \
                precision, so Bynk makes you write it: `.toFloat()` on an `Int`, \
                `.round()`, `.floor()`, `.ceil()` or `.truncate()` on a `Float`, \
                `Decimal.fromInt(n)` or a decimal literal for a `Decimal`. The same \
                code covers a `Duration` or `Instant` used with an operand it does not \
                support.",
        example: "fn total(count: Int, price: Float) -> Float {\n    count * price                    // ✗ `Int` and `Float` in one operator\n}\n\n\
                  fn total(count: Int, price: Float) -> Float {\n    count.toFloat() * price          // ✓ explicit conversion\n}",
        page: "/book/reference/operators",
        anchor: "",
    },
    Explain {
        code: "bynk.types.non_exhaustive_match",
        blurb: "A `match` must cover every value its subject can take, so it can never \
                fall off its end. The compiler names a case that no arm covers: add an \
                arm for it, or end the `match` with a wildcard `_` arm. Listing every \
                variant a type has today is not a substitute for `_` where the compiler \
                says one is needed, and where a `match` is too complex for it to prove \
                exhaustive it asks for `_` too.",
        example: "type Status = enum { Pending, Active, Done }\n\n\
                  match s {                // ✗ `Done` is not covered\n    Pending => \"p\"\n    Active => \"a\"\n}\n\n\
                  match s {\n    Pending => \"p\"\n    Active => \"a\"\n    Done => \"d\"        // ✓ every variant covered\n}",
        page: "/book/reference/types",
        anchor: "exhaustiveness-and-reachability",
    },
    Explain {
        code: "bynk.types.not_codable",
        blurb: "A type parameter declared `T: Codable`, on a generic `fn` or a capability \
                operation, accepts only the types `Json.decode` can read. A call that makes `T` \
                a function, an `Effect`, a `Stream` or a type carrying one, a `bynk.crypto` \
                key or `()` is refused, because such a value has no wire form. Pass a plain \
                value, or use a function whose parameter is not `Codable`. Calling \
                `Json.encode` or `Json.decode` on such a type directly is \
                `bynk.types.json_uncodable`.",
        example: "fn keep[T: Codable](x: T) -> T { x }\n\n\
                  fn run(f: Int -> Int) -> Int {\n    let _ = keep(f)    // ✗ a function has no wire form\n    1\n}\n\n\
                  fn run(n: Int) -> Int {\n    keep(n)    // ✓ an `Int` does\n}",
        page: "/book/reference/generics",
        anchor: "bounds",
    },
    Explain {
        code: "bynk.types.not_comparable",
        blurb: "`==` and `!=` compare values structurally, which needs a value to compare. \
                A function, an `Effect` and a `Query` are computations, and two of them have no \
                equality. A type that holds one anywhere inside it, in a field, a payload, an \
                element or a generic argument, is refused too. Compare what the computation \
                produces, or the data it was built from. A `Stream` and a held connection have \
                their own codes, `bynk.types.stream_not_comparable` and \
                `bynk.types.held_not_comparable`.",
        example: "fn same(f: Int -> Int, g: Int -> Int) -> Bool {\n    f == g          // ✗ functions have no equality\n}\n\n\
                  fn same(f: Int -> Int, g: Int -> Int) -> Bool {\n    f(1) == g(1)    // ✓ compare what they produce\n}",
        page: "/book/reference/operators",
        anchor: "",
    },
    Explain {
        code: "bynk.types.not_keyable",
        blurb: "A `Map` key, a `Set` element, or the key of a `distinct`, `distinctBy`, `groupBy` \
                or join, must be a `Key` type, one whose equal values can always be found \
                again. `String`, `Int`, `Bool`, `Instant`, `Duration` and `Bytes` are, as are \
                `Option` and `List` of a key and records, sums and enums made only of keys. \
                A `Float` never is, at any depth, because `NaN` is not equal to itself, so a \
                key holding one could never be looked up. Key on a type that is: an `Int` \
                count of cents, say, or a `String`.",
        example: "fn distinctPrices(xs: List[Float]) -> Int {\n    let seen: Set[Float] = Set.of(xs)    // ✗ `Float` is not a `Key`\n    seen.length()\n}\n\n\
                  fn distinctPrices(xs: List[Int]) -> Int {\n    let seen: Set[Int] = Set.of(xs)      // ✓ `Int` is\n    seen.length()\n}",
        page: "/book/reference/types",
        anchor: "keys",
    },
    Explain {
        code: "bynk.types.not_orderable",
        blurb: "`<`, `<=`, `>` and `>=`, and a parameter declared `T: Ord`, need a type with \
                a total order. `Int`, `Float`, `String`, `Duration`, `Instant` and `Bytes` have \
                one, as do refined types over them, `Option` and `List` of an orderable type, \
                and any type of your own that declares `derives Ord`. `Bool`, `Result`, `Map`, \
                `Set` and functions do not. Your own types are not ordered by accident, \
                because opting in makes declaration order significant: reordering the fields \
                or variants changes how values sort. Add `derives Ord`, or compare a field \
                that is already orderable. A `sortBy`, `min` or `max` key that cannot be \
                ordered is `bynk.types.key_not_orderable`.",
        example: "type Point = { x: Int, y: Int }\n\n\
                  fn before(a: Point, b: Point) -> Bool {\n    a < b    // ✗ `Point` does not derive `Ord`\n}\n\n\
                  type Point = { x: Int, y: Int } derives Ord    // ✓ ordered by `x`, then `y`",
        page: "/book/reference/types",
        anchor: "ordering-your-own-types",
    },
    Explain {
        code: "bynk.types.ok_value_mismatch",
        blurb: "`Ok(v)` takes its type from where it is used. A return type or annotation of `Result[T, E]` fixes `T`, so `Ok(\"12\")` where `T` is `Int` is refused at the value, with no conversion; so is a value that does not match an `HttpResult[T]` handler's `T`. Pass a `T`, or change the declared type. The error half is `bynk.types.err_value_mismatch`; with nothing to fix `T`, it is `bynk.types.cannot_infer_result_type_params`.",
        example: "fn total() -> Result[Int, String] {\n    Ok(\"12\")    // ✗ a `String`, where `Int` is expected\n}\n\nfn total() -> Result[Int, String] {\n    Ok(12)    // ✓\n}",
        page: "/book/guides/type-system/result-and-optionals",
        anchor: "construct-values",
    },
    Explain {
        code: "bynk.types.opaque_raw_outside",
        blurb: "An opaque type hides its base representation from everything but the unit that declares it, so no other code can take a `Token` apart or forge one. `.raw` reads the base value and is allowed only there. Elsewhere, use what the defining unit offers: a method or accessor on the type. To make a value, use `Token.of(value)`.",
        example: "// token.bynk\ntype Token = opaque String\n\nfn Token.value(self) -> String {\n    self.raw\n}\n\n// app.bynk\nfn show(t: Token) -> String {\n    t.raw    // ✗ `Token` is opaque outside `token`\n}\n\nfn show(t: Token) -> String {\n    t.value()    // ✓ use the accessor `token` provides\n}",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.types.opaque_unsafe_outside",
        blurb: "`T.unsafe(v)` wraps a base value as an opaque type without checking it, which is sound only where the type's own unit vouches for the value, so it is allowed only in the unit that declares the type. Elsewhere use `T.of(v)`, which returns a `Result` so the check is explicit. Reading the base value back is `bynk.types.opaque_raw_outside`.",
        example: "// token.bynk\ntype Token = opaque String\n\n// app.bynk\nfn make(s: String) -> Token {\n    Token.unsafe(s)    // ✗ only `token` may skip the check\n}\n\nfn make(s: String) -> Result[Token, ValidationError] {\n    Token.of(s)    // ✓ the checked constructor\n}",
        page: "/book/reference/types",
        anchor: "opaque-types",
    },
    Explain {
        code: "bynk.types.or_pattern_binding_mismatch",
        blurb: "An or-pattern, `Walk(steps) | Wait`, runs one arm for either alternative, so every alternative must bind the same names. If `steps` came only from `Walk`, the arm's body would read a name that does not exist when `Wait` matched. Bind the name in every alternative, bind it in none with `_`, or split the alternatives into separate arms. A shared name must also have the same type: see `bynk.types.or_pattern_type_mismatch`.",
        example: "type Move = | Walk(steps: Int) | Wait\n\nfn cost(m: Move) -> Int {\n    match m {\n        Walk(steps) | Wait => 1    // ✗ `steps` is bound by one alternative only\n    }\n}\n\nfn cost(m: Move) -> Int {\n    match m {\n        Walk(_) | Wait => 1        // ✓ neither alternative binds\n    }\n}",
        page: "/book/guides/type-system/match",
        anchor: "",
    },
    Explain {
        code: "bynk.types.or_pattern_type_mismatch",
        blurb: "An or-pattern, `A(n) | B(n)`, runs one arm for either alternative, so a name it binds must mean the same thing in both: the arm's body has only one `n` to use. Here `n` is an `Int` in one alternative and a `String` in the other, so the body could not be checked. A refined type and its base count as different too. Give each alternative its own arm, or bind a value of the same type in both.",
        example: "type Key = | Id(n: Int) | Name(n: String)\n\nfn show(k: Key) -> String {\n    match k {\n        Id(n) | Name(n) => \"key\"    // ✗ `n` is an `Int` and a `String`\n    }\n}\n\nfn show(k: Key) -> String {\n    match k {\n        Id(n) => \"id\"               // ✓ one arm each\n        Name(n) => n\n    }\n}",
        page: "/book/guides/type-system/match",
        anchor: "",
    },
    Explain {
        code: "bynk.types.pattern_arity",
        blurb: "A variant pattern takes one binding for each payload field the variant \
                declares, in order, or none to ignore the payload. A different count, or any \
                binding on a variant with no payload, is refused: an extra name has nothing to \
                hold, and a missing one would silently drop a field. Use `_` for a field you \
                do not need, or name the fields you want (`radius: r`). Building a variant \
                with the wrong number of arguments is `bynk.types.variant_arity`.",
        example: "type Shape =\n  | Circle(radius: Int)\n  | Dot\n\n\
                  fn size(s: Shape) -> Int {\n    match s {\n        Circle(r, extra) => r    // ✗ `Circle` has one field\n        Dot => 0\n    }\n}\n\n\
                  fn size(s: Shape) -> Int {\n    match s {\n        Circle(r) => r           // ✓\n        Dot => 0\n    }\n}",
        page: "/book/reference/types",
        anchor: "matching",
    },
    Explain {
        code: "bynk.types.pattern_type_mismatch",
        blurb: "A pattern can only match values of the type it describes, so a pattern of \
                another kind than the value being matched can never succeed and is refused. \
                A literal pattern must have the scrutinee's type, so `\"one\"` cannot match an \
                `Int`. A variant pattern needs a sum value, a record pattern needs that \
                record, and a qualified pattern such as `Status.Active` must name the \
                scrutinee's own type. Match on the type the value has, or convert the value \
                before the `match`. A pattern with the wrong number of bindings is \
                `bynk.types.pattern_arity`.",
        example: "fn word(n: Int) -> String {\n    match n {\n        \"one\" => \"uno\"    // ✗ a `String` pattern, an `Int` scrutinee\n        _ => \"other\"\n    }\n}\n\n\
                  fn word(n: Int) -> String {\n    match n {\n        1 => \"uno\"        // ✓\n        _ => \"other\"\n    }\n}",
        page: "/book/reference/types",
        anchor: "matching",
    },
    Explain {
        code: "bynk.types.polynomial_regex",
        blurb: "A `Matches` pattern runs under a backtracking `RegExp` on untrusted input. Two \
                repetitions in sequence that can match the same characters, as in `\\d*\\d*`, \
                let the engine split a run between them many ways, so the time grows as a \
                power of the input's length. That is slower to hurt than \
                `bynk.types.catastrophic_regex` but can still stall a handler. A `MaxLength`, \
                `Length` or `IsoDate` predicate that keeps the input short enough makes it \
                the warning `bynk.types.polynomial_regex_capped`; a regex method has no such \
                bound. Rewrite it so the repetitions cannot overlap.",
        example: "type Digits = String where Matches(\"\\\\d*\\\\d*\")    // ✗ two loops over the same digits\n\n\
                  type Digits = String where Matches(\"\\\\d*\")         // ✓ one way to match",
        page: "/book/reference/types",
        anchor: "regular-expressions",
    },
    Explain {
        code: "bynk.types.predicate_base_mismatch",
        blurb: "Each predicate checks one kind of value, so it can only refine a base that has such values. `NonEmpty`, `MinLength`, `MaxLength`, `Length`, `Matches` and `IsoDate` apply to a `String`, and `NonNegative`, `Positive` and `InRange` to an `Int`, `Float` or `Decimal`. A mismatch would test something the base cannot have. Change the base type to suit the predicate, or choose the predicate that suits the base.",
        example: "type Code = Int where NonEmpty         // ✗ `NonEmpty` is a `String` predicate\n\ntype Code = String where NonEmpty      // ✓",
        page: "/book/reference/refined-types",
        anchor: "predicates",
    },
    Explain {
        code: "bynk.types.query_at_boundary",
        blurb: "A `Query[T]` is a lazy read over an agent's stored data: nothing runs until a terminal such as `.collect()` executes it, in the agent that owns the data. Persisted or sent, it would be a description of a read with nothing to read from. A helper may take or return one, but it is refused as a record, sum or event field, an agent key or store field, a capability operation, or a handler's parameter or result. Run it and keep or send the `List` or `Option` it gives. A `Stream` has its own rule: `bynk.types.stream_at_boundary`.",
        example: "type Order = { id: String, total: Int }\n\ntype Report = {\n    orders: Query[Order],    // ✗ a lazy read is not data\n}\n\ntype Report = {\n    orders: List[Order],     // ✓ run the query, keep its result\n}",
        page: "/book/reference/types",
        anchor: "query",
    },
    Explain {
        code: "bynk.types.question_error_mismatch",
        blurb: "`?` returns the `Err` from the enclosing function as it is, so its error type must be one the function declares: a `String` error cannot escape a function whose errors are `Int`. The only automatic conversion is a declared embedding, where the function's error is a sum type with an `embeds` clause for the inner one. Otherwise convert the error before the `?` with `.mapErr(…)`, or change the function's error type.",
        example: "fn checked(n: Int) -> Result[Int, String] {\n    if n >= 0 { Ok(n) } else { Err(\"negative\") }\n}\n\nfn run(n: Int) -> Result[Int, Int] {\n    let v = checked(n)?    // ✗ propagates a `String` error, but `run` returns `Int` errors\n    Ok(v + 1)\n}\n\nfn run(n: Int) -> Result[Int, Int] {\n    let v = checked(n).mapErr((e) => e.length())?    // ✓ the error is converted first\n    Ok(v + 1)\n}",
        page: "/book/guides/type-system/result-and-optionals",
        anchor: "",
    },
    Explain {
        code: "bynk.types.question_on_non_result",
        blurb: "`?` unwraps an `Ok` and returns early from the function on an `Err`, so it needs a `Result[T, E]`: a plain `Int` has no error to return. The one other operand it takes is an `Option`, in an `HttpResult` handler. Apply `?` to the call that produces the `Result`, not to a value already unwrapped. To turn an `Option` into a `Result` elsewhere, use `.okOr(err)`.",
        example: "fn checked(n: Int) -> Result[Int, String] {\n    if n >= 0 { Ok(n) } else { Err(\"negative\") }\n}\n\nfn run(n: Int) -> Result[Int, String] {\n    let v = n?    // ✗ `n` is an `Int`, not a `Result`\n    Ok(v + 1)\n}\n\nfn run(n: Int) -> Result[Int, String] {\n    let v = checked(n)?    // ✓\n    Ok(v + 1)\n}",
        page: "/book/guides/type-system/result-and-optionals",
        anchor: "",
    },
    Explain {
        code: "bynk.types.question_option_outside_http",
        blurb: "`?` on an `Option` turns `None` into an early return, which needs somewhere to return it to. In a handler that returns an `HttpResult`, `None` becomes a `NotFound` response. Elsewhere an `Option` has no error to carry, so `?` is refused. Say what absence means with `.okOr(err)`, which makes a `Result`, and put `?` after that.",
        example: "fn lookup(id: Int) -> Option[Int] { … }\n\nfn double(id: Int) -> Result[Int, String] {\n    let n = lookup(id)?                    // ✗ no `HttpResult` to answer `NotFound`\n    Ok(n * 2)\n}\n\nfn double(id: Int) -> Result[Int, String] {\n    let n = lookup(id).okOr(\"missing\")?    // ✓ `None` becomes an `Err`\n    Ok(n * 2)\n}",
        page: "/book/guides/type-system/result-and-optionals",
        anchor: "methods",
    },
    Explain {
        code: "bynk.types.question_outside_result",
        blurb: "`?` returns early with the `Err`, so the function around it must be able to return one: its return type has to be a `Result`, or an `Effect` of one. In a function returning `Int` the error has nowhere to go. Change the return type to `Result[T, E]` and wrap the final value in `Ok`, or deal with the `Result` where it arises, with a `match` or `.getOrElse(…)`.",
        example: "fn checked(n: Int) -> Result[Int, String] {\n    if n >= 0 { Ok(n) } else { Err(\"negative\") }\n}\n\nfn run(n: Int) -> Int {\n    let v = checked(n)?    // ✗ `run` does not return a `Result`\n    v + 1\n}\n\nfn run(n: Int) -> Result[Int, String] {\n    let v = checked(n)?    // ✓\n    Ok(v + 1)\n}",
        page: "/book/guides/type-system/result-and-optionals",
        anchor: "",
    },
    Explain {
        code: "bynk.types.range_pattern_on_non_int",
        blurb: "A range pattern `lo..hi` matches an `Int` by comparing it with two bounds. It is not offered for a `Float`, because `NaN` is neither inside nor outside a range, nor for a `String`; both are left out on purpose. Compare with `<` and `>=` in an `if` instead. An empty range is `bynk.types.empty_range_pattern`.",
        example: "fn band(x: Float) -> String {\n    match x {\n        1..3 => \"low\"             // ✗ a range pattern matches an `Int`\n        _ => \"high\"\n    }\n}\n\nfn band(x: Float) -> String {\n    if x < 3.0 { \"low\" } else { \"high\" }    // ✓ compare in an `if`\n}",
        page: "/book/reference/types",
        anchor: "range-patterns",
    },
    Explain {
        code: "bynk.types.record_pattern_on_non_record",
        blurb: "A record pattern, `Order { total, .. }`, takes a record apart by its fields, so the name before the braces must be a record type. A sum, an enum, a refined or opaque type and a built-in have no fields to name. Match a sum or an enum by its variants, and read an opaque or built-in value through its methods.",
        example: "type Status = enum { Open, Paid }\n\nfn code(s: Status) -> Int {\n    match s {\n        Status { .. } => 1    // ✗ `Status` is an enum, not a record\n        _ => 2\n    }\n}\n\nfn code(s: Status) -> Int {\n    match s {\n        Open => 1             // ✓ name the variants\n        Paid => 2\n    }\n}",
        page: "/book/reference/types",
        anchor: "record-patterns",
    },
    Explain {
        code: "bynk.types.regex_not_literal",
        blurb: "A regex method checks its pattern when the program compiles, to reject one that can take catastrophic time on crafted input (ReDoS). A pattern built at run time would escape that check, so it must be a string literal. Write the pattern in place, as an ordinary or a raw string, `r\"\\d+\"`. To look for text that varies, use `contains`, which takes plain text.",
        example: "fn has(s: String, pattern: String) -> Bool {\n    s.containsRegex(pattern)              // ✗ the pattern is not a literal\n}\n\nfn has(s: String) -> Bool {\n    s.containsRegex(\"(?i)error\")          // ✓\n}",
        page: "/book/reference/types",
        anchor: "regular-expressions",
    },
    Explain {
        code: "bynk.types.rejection_at_boundary",
        blurb: "A `BoundaryRejection` is the router's account of a request it refused before any handler ran: a malformed or oversized body, an unsupported media type, a wrong method. `on rejected` reads it to render the answer, and it is accepted only as that handler's parameter. It is refused as a record, sum or event field, an agent store field, a capability operation, or any other handler's parameter or result. Match on it in `on rejected` and keep what you need, such as the detail text, as a `String`.",
        example: "type Refusal = {\n    why: BoundaryRejection,    // ✗ read by `on rejected`, not kept\n}\n\ntype Refusal = {\n    detail: String,            // ✓ keep what you need from it\n}",
        page: "/book/reference/http",
        anchor: "rendering-rejections",
    },
    Explain {
        code: "bynk.types.request_cookies_at_boundary",
        blurb: "`RequestCookies` is the raw view of one request's cookies, the `cookies` parameter of a handler that needs a name a record field cannot spell, such as `__Host-sid`. Cookies often carry session credentials, so the view is read where the request arrives and never kept. It is accepted only as an HTTP handler's `cookies` parameter, and refused as a record, sum or event field, an agent key or store field, a capability operation, or any other handler parameter or result. Read the cookie you need (`cookies.get(\"sid\")`) and keep that `String`.",
        example: "type Session = {\n    cookies: RequestCookies,    // ✗ a request's cookies are not data to keep\n}\n\ntype Session = {\n    sid: String,                // ✓ keep the value you read\n}",
        page: "/book/reference/http",
        anchor: "request-cookies",
    },
    Explain {
        code: "bynk.types.request_query_at_boundary",
        blurb: "`RequestQuery` is the raw view of one request's query string, the `query` parameter of a handler that reads keys it does not know in advance. It is read where the request arrives and then dropped: a copy kept or sent on would outlive the request. It is accepted only as an HTTP handler's `query` parameter, and refused as a record, sum or event field, an agent key or store field, a capability operation, or any other handler parameter or result. Read the value you need (`query.get(\"q\")`) and keep that `String`.",
        example: "type Search = {\n    query: RequestQuery,    // ✗ a request's query string is not data to keep\n}\n\ntype Search = {\n    term: String,           // ✓ keep the value you read\n}",
        page: "/book/reference/http",
        anchor: "request-query",
    },
    Explain {
        code: "bynk.types.return_mismatch",
        blurb: "The value a function or handler body ends with does not have the declared \
                return type, and the compiler compares the two exactly. Either change the \
                body so it produces the declared type, or change the declared return type \
                to match what the body produces. A refined type is the same rule with one extra step: an \
                `Int` is not an `Age`, because `Age` carries a rule the `Int` has not been \
                checked against. Build the refined value with `.of`, which checks the \
                predicate and returns a `Result`, and declare that `Result` as the return \
                type.",
        example: "fn count() -> Int { \"three\" }       // ✗ a `String` where an `Int` is declared\n\n\
                  fn count() -> Int { 3 }              // ✓ the body has the declared type\n\n\
                  type Age = Int where InRange(0, 150)\n\n\
                  fn parse(n: Int) -> Age { n }                           // ✗ an `Int` is not an `Age`\n\n\
                  fn parse(n: Int) -> Result[Age, ValidationError] {      // ✓ checked by `.of`\n    Age.of(n)\n}",
        page: "/book/reference/types",
        anchor: "",
    },
    Explain {
        code: "bynk.types.rounding_mode_position",
        blurb: "A rounding mode is written `RoundingMode.<mode>` where the rounding happens, as the mode argument of a `Decimal`'s `round` or `div`. It is not a value: it cannot be bound to a name, stored or passed around, so the rounding at each call is visible at that call. Write the mode at the call. A mode that is not a `RoundingMode.<mode>` at all is `bynk.types.unknown_rounding_mode`.",
        example: "let mode = RoundingMode.HalfUp         // ✗ a mode is not a value\n\nprice.round(2, RoundingMode.HalfUp)    // ✓ written where it rounds",
        page: "/book/reference/types",
        anchor: "decimal",
    },
    Explain {
        code: "bynk.types.select_tail_has_select",
        blurb: "`select[T]` and `firstAs[T]` write the `SELECT` and the column list themselves, from the fields of the row record `T`, so the SQL you give is only the rest, starting at `FROM`. That way the columns are spelt once and cannot drift from the type. Start at `FROM`. A query that cannot (a `WITH` or a parenthesised one) is read with `all` and `Json.decodeSql`, which take the whole statement.",
        example: "let _ <- Orders.select[Order](\"SELECT id, total FROM orders\", [])    // ✗ the compiler writes `SELECT`\n\nlet _ <- Orders.select[Order](\"FROM orders\", [])                      // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "d1-typed-reads",
    },
    Explain {
        code: "bynk.types.sequence_needs_result",
        blurb: "`sequenceAll` and `sequenceTry` fold a `List[Result[T, E]]` into one `Result`, so every element must already be a `Result`. A list of plain values has no failures to collect. Map each element through a function that returns a `Result` first, as in `xs.map(check).sequenceAll()`. `sequenceAll` gathers every error, and `sequenceTry` stops at the first.",
        example: "fn check(n: Int) -> Result[Int, String] { … }\n\nfn all(xs: List[Int]) -> Result[List[Int], List[String]] {\n    xs.sequenceAll()                // ✗ the elements are `Int`s, not `Result`s\n}\n\nfn all(xs: List[Int]) -> Result[List[Int], List[String]] {\n    xs.map(check).sequenceAll()     // ✓\n}",
        page: "/book/reference/types",
        anchor: "list-methods",
    },
    Explain {
        code: "bynk.types.some_value_mismatch",
        blurb: "`Some(v)` takes its type from the surrounding `Option[T]`, the annotation or the function's return type, so `Some(\"a\")` where `T` is `Int` is refused at the value, with no conversion. Pass a `T`, or change the declared type. With nothing to say what the `Option` holds, a bare `None` is `bynk.types.cannot_infer_option_type_param`.",
        example: "fn first() -> Option[Int] {\n    Some(\"a\")    // ✗ a `String`, where `Int` is expected\n}\n\nfn first() -> Option[Int] {\n    Some(1)    // ✓\n}",
        page: "/book/guides/type-system/result-and-optionals",
        anchor: "construct-values",
    },
    Explain {
        code: "bynk.types.sql_not_literal",
        blurb: "The SQL of a `D1`, `Postgres` or `MySql` call must be a string literal, or a `String` constant bound to one, so it is fixed when the program compiles and no request data can become part of the statement: that is how SQL injection happens. An interpolated string, a parameter or a `let` binding is refused. Put the value in the parameter list, as `?1` with `Text(id)`, which is always bound and never spliced in.",
        example: "let _ <- Orders.all(\"SELECT id FROM orders WHERE id = '\\(id)'\", [])         // ✗ built at run time\n\nlet _ <- Orders.all(\"SELECT id FROM orders WHERE id = ?1\", [Text(id)])      // ✓ bound",
        page: "/book/reference/bynk-capabilities",
        anchor: "sql-databases-d1",
    },
    Explain {
        code: "bynk.types.sql_row_shape",
        blurb: "`select[T]`, `firstAs[T]` and `Json.decodeSql` read each SQL row into a record, and a column holds only a plain value. So `T` must be a record whose fields are `Int`, `Float`, `String`, `Bool` or `Bytes`, or a refined or opaque type over one, or an `Option` of either (Postgres and MySQL also read `Decimal`); a nested record, a list or an enum has no column to read. `T` is the row itself, so `select[List[Row]]` is refused too. Flatten the record.",
        example: "type Money = { cents: Int }\ntype Order = { id: String, total: Money }\nlet _ <- Orders.select[Order](\"FROM orders\", [])    // ✗ `total` is a record\n\ntype Order = { id: String, total: Int }\nlet _ <- Orders.select[Order](\"FROM orders\", [])    // ✓",
        page: "/book/reference/bynk-capabilities",
        anchor: "d1-typed-reads",
    },
    Explain {
        code: "bynk.types.sql_statement_at_boundary",
        blurb: "A `SqlStatement`, `PgStatement` or `MySqlStatement` is a statement a database capability runs, whose SQL the compiler requires to be a literal, so that request data is only ever bound as a parameter. One restored from storage or received from a caller would carry SQL that was never a literal, so it is refused as a record, sum or event field, an agent store field, a handler parameter or result, or a `Json` codec target. A capability operation's signature may name one. Keep or send the values instead, and build the statement where it runs.",
        example: "type Job = {\n    statement: SqlStatement,    // ✗ SQL that was never a literal\n}\n\ntype Job = {\n    id: String,                 // ✓ send the value, build the statement where it runs\n}",
        page: "/book/reference/bynk-capabilities",
        anchor: "sql-databases-d1",
    },
    Explain {
        code: "bynk.types.stream_at_boundary",
        blurb: "A `Stream[T]` is a live, lazily read source, built and consumed where it is \
                made. It cannot be persisted or serialised, so it cannot be a service or agent \
                handler parameter or result, a store field or an event field, nor can a record \
                or sum that carries one in a field, at any depth. Drain it with `collect()` \
                and send or keep the resulting `List`. A capability operation may take or \
                return a stream, since its provider runs in the caller's own isolate, and a \
                `@raw` HTTP handler's `body` may be a `Stream[Bytes]`, the request body as it \
                arrives.",
        example: "service s {\n    on call(readings: Stream[Int]) -> Effect[Int] {    // ✗ a stream cannot be a parameter\n        Effect.pure(0)\n    }\n}\n\n\
                  service s {\n    on call(readings: List[Int]) -> Effect[Int] {      // ✓\n        Effect.pure(readings.length())\n    }\n}",
        page: "/book/reference/types",
        anchor: "stream",
    },
    Explain {
        code: "bynk.types.stream_not_comparable",
        blurb: "A `Stream` is a source of values over time, read once, so two of them have no value to compare. `==` on one, or on a type that holds one, is refused. Read the streams first, with `collect`, and compare the lists, or compare the data each was built from. Functions and effects have `bynk.types.not_comparable`, and a held connection has `bynk.types.held_not_comparable`.",
        example: "fn same(a: Stream[Int], b: Stream[Int]) -> Bool {\n    a == b                            // ✗ a stream has no value to compare\n}\n\nfn same(a: Stream[Int], b: Stream[Int]) -> Effect[Bool] {\n    let xs <- a.collect()             // ✓ read them first\n    let ys <- b.collect()\n    Effect.pure(xs == ys)\n}",
        page: "/book/reference/types",
        anchor: "stream",
    },
    Explain {
        code: "bynk.types.type_mismatch",
        blurb: "An operator or a built-in method was given an operand of a type it does not \
                accept: `==` between an `Int` and a `String`, `!` on a number, `&&` on \
                something that is not a `Bool`. Types are compared exactly, with no implicit \
                conversion, so values of different types are never equal and never silently \
                coerced. Make the operands the same type, or convert one explicitly. A `Float` \
                and an `Int` mixed in arithmetic is the separate code \
                `bynk.types.no_numeric_coercion`.",
        example: "fn isAdult(age: Int) -> Bool {\n    age == \"18\"    // ✗ `Int` compared with `String`\n}\n\n\
                  fn isAdult(age: Int) -> Bool {\n    age == 18      // ✓ both `Int`\n}",
        page: "/book/reference/operators",
        anchor: "",
    },
    Explain {
        code: "bynk.types.uninferable_element_type",
        blurb: "An empty `[]`, `List.empty()`, `Set.empty()` or `Map.empty()` has no elements to \
                take a type from, and the compiler will not guess, because the element type \
                becomes part of the type of everything the collection touches. Give it one: \
                annotate the binding, as `let xs: List[Int] = []`, or use the empty value \
                where a type is expected, such as an argument or a return value.",
        example: "fn count() -> Int {\n    let xs = []                  // ✗ a list of what?\n    xs.length()\n}\n\n\
                  fn count() -> Int {\n    let xs: List[Int] = []       // ✓\n    xs.length()\n}",
        page: "/book/reference/types",
        anchor: "built-in-generic-types",
    },
    Explain {
        code: "bynk.types.unknown_annotation",
        blurb: "After a refinement, the only annotation Bynk knows is `@example(\"…\")`, a sample value that the compiler checks against the refinement. Any other name is refused rather than ignored, so a misspelling cannot pass as a comment. Use `@example`, or delete the annotation. `@example` is accepted only on `Matches` and `IsoDate`.",
        example: "type Code = String where Matches(\"[A-Z]{3}\") @sample(\"ABC\")     // ✗ no annotation `@sample`\n\ntype Code = String where Matches(\"[A-Z]{3}\") @example(\"ABC\")    // ✓",
        page: "/book/reference/refined-types",
        anchor: "predicates",
    },
    Explain {
        code: "bynk.types.unknown_field",
        blurb: "A field access names a field the value's type does not declare. A record's \
                fields are fixed by its type declaration, so `p.z` on a `Point` of `x` and `y` \
                is refused. This is usually a typo, or a value of a different type from the \
                one intended. Building a record with a field it lacks is the sibling code \
                `bynk.resolve.unknown_field`.",
        example: "type Point = { x: Int, y: Int }\n\n\
                  fn height(p: Point) -> Int {\n    p.z    // ✗ `Point` has no field `z`\n}\n\n\
                  fn height(p: Point) -> Int {\n    p.y    // ✓\n}",
        page: "/book/reference/types",
        anchor: "record-types",
    },
    Explain {
        code: "bynk.types.unknown_pattern_field",
        blurb: "When a variant pattern names its bindings, as in `Circle(radius: r)`, each name must be a payload field the variant declares: `Circle` has no `diameter`. Use the declared field name, or bind by position, `Circle(r)`. A count of bindings that does not fit the payload is `bynk.types.pattern_arity`.",
        example: "type Shape =\n  | Circle(radius: Int)\n  | Dot\n\nfn size(s: Shape) -> Int {\n    match s {\n        Circle(diameter: d) => d    // ✗ `Circle` has no field `diameter`\n        Dot => 0\n    }\n}\n\nfn size(s: Shape) -> Int {\n    match s {\n        Circle(radius: r) => r    // ✓\n        Dot => 0\n    }\n}",
        page: "/book/reference/types",
        anchor: "matching",
    },
    Explain {
        code: "bynk.types.unknown_rounding_mode",
        blurb: "`Decimal`'s `round` and `div` must be told how to break a tie, and the choice is part of the money arithmetic, so it is named, not defaulted: write `RoundingMode.<mode>` as the last argument, where the mode is one of `Down`, `Up`, `Floor`, `Ceiling`, `HalfUp`, `HalfDown` or `HalfEven`. A number, a string or a variable is refused. A mode kept in a binding is `bynk.types.rounding_mode_position`.",
        example: "price.round(2, 1)                        // ✗ not a rounding mode\n\nprice.round(2, RoundingMode.HalfEven)    // ✓",
        page: "/book/reference/types",
        anchor: "decimal",
    },
    Explain {
        code: "bynk.types.unknown_static_member",
        blurb: "`HttpResult` is a built-in type with a fixed set of variants, the HTTP statuses a handler can answer with, such as `Ok`, `Created`, `NotFound` and `ServerError`. `HttpResult.Nope` names one that does not exist, usually a misspelling or a status the set does not include. Pick one of the listed variants. A type you declared yourself is checked by `bynk.resolve.unknown_static_member`.",
        example: "HttpResult.Nope        // ✗ no such variant\n\nHttpResult.NotFound    // ✓",
        page: "/book/reference/http",
        anchor: "httpresult-variants",
    },
    Explain {
        code: "bynk.types.unknown_variant_in_pattern",
        blurb: "A pattern on a sum value can only name the variants its type declares, so `Square(side)` against a `Shape` with no `Square` could never match and is refused. This is usually a typo, a variant that was never added, or a value of a different type from the one intended. Fix the name, add the variant to the type, or match on the right value. A pattern of the wrong kind altogether is `bynk.types.pattern_type_mismatch`.",
        example: "type Shape =\n  | Circle(radius: Int)\n  | Dot\n\nfn size(s: Shape) -> Int {\n    match s {\n        Circle(r) => r\n        Square(side) => side    // ✗ `Shape` has no `Square`\n        Dot => 0\n    }\n}\n\nfn size(s: Shape) -> Int {\n    match s {\n        Circle(r) => r\n        Dot => 0    // ✓\n    }\n}",
        page: "/book/reference/types",
        anchor: "matching",
    },
    Explain {
        code: "bynk.types.unreachable_arm",
        blurb: "A `match` arm can never run, because the arms before it already match every \
                value it would match. The usual cause is a wildcard `_` placed before more \
                specific arms: arms are tried in order, so the wildcard takes everything and \
                the rest are dead code, which Bynk refuses rather than let hide a mistake. \
                Put the specific arms first and the wildcard last, or delete the arm. A \
                guarded arm covers nothing, since its guard may fail.",
        example: "type Status = enum { Pending, Active, Done }\n\n\
                  fn label(s: Status) -> String {\n    match s {\n        _ => \"other\"\n        Pending => \"waiting\"    // ✗ the `_` above already matches it\n    }\n}\n\n\
                  fn label(s: Status) -> String {\n    match s {\n        Pending => \"waiting\"\n        _ => \"other\"            // ✓ the catch-all last\n    }\n}",
        page: "/book/reference/types",
        anchor: "exhaustiveness-and-reachability",
    },
    Explain {
        code: "bynk.types.variant_arity",
        blurb: "A variant is built like a call, with one argument for each payload field it \
                declares, so `Circle(radius: Int)` takes one and a variant with no payload \
                is written bare. A different number of arguments is refused at the \
                construction. Pass one value for each declared field, or change the type \
                declaration if the variant should carry more. The same code covers the \
                built-in `QueueResult` and `HttpResult` constructors. A `match` arm with the \
                wrong number of bindings is the separate code `bynk.types.pattern_arity`.",
        example: "type Shape =\n  | Circle(radius: Int)\n  | Square(side: Int)\n\n\
                  fn make() -> Shape {\n    Circle(1, 2)    // ✗ `Circle` takes 1 argument\n}\n\n\
                  fn make() -> Shape {\n    Circle(1)       // ✓\n}",
        page: "/book/reference/types",
        anchor: "sum-types",
    },
    Explain {
        code: "bynk.types.variant_missing_payload",
        blurb: "A variant with a payload is built by calling it, with its payload as arguments. Written bare, `Circle` is not a value, since nothing fills `radius`; only a variant with no payload can stand alone. Call it with one argument per field, as `Circle(10)`. The qualified form `Shape.Circle` is refused in the same way. A call with the wrong number of arguments is `bynk.types.variant_arity`.",
        example: "type Shape =\n  | Circle(radius: Int)\n  | Dot\n\nfn make() -> Shape {\n    Circle    // ✗ `Circle` carries a payload\n}\n\nfn make() -> Shape {\n    Circle(10)    // ✓\n}",
        page: "/book/reference/types",
        anchor: "sum-types",
    },
    Explain {
        code: "bynk.types.variant_payload_mismatch",
        blurb: "A variant's payload fields have declared types and are checked like function parameters: `Circle(\"big\")` for `radius: Int` is refused at the argument, with no conversion. Pass a value of the field's type. For a generic sum the type argument is inferred from the arguments first, and the check applies to the result. The wrong number of arguments is `bynk.types.variant_arity`.",
        example: "type Shape =\n  | Circle(radius: Int)\n  | Dot\n\nfn make() -> Shape {\n    Circle(\"big\")    // ✗ `radius` is an `Int`\n}\n\nfn make() -> Shape {\n    Circle(10)    // ✓\n}",
        page: "/book/reference/types",
        anchor: "sum-types",
    },
    Explain {
        code: "bynk.uses.name_conflict",
        blurb: "A unit that `uses` a commons gets what it declares (types, functions, constants, assets \
                and embeds) in scope under their own names, with no prefix. If two commons it uses \
                both declare the same name, a mention of it could mean either, so the compiler \
                refuses the second `uses`. `uses` has no `as` rename yet. Rename one of the \
                declarations, drop one of the `uses`, or mark the declaration `private` if only \
                its own commons needs it, since a `private` declaration is never imported and so \
                never conflicts.",
        example: "// billing.bynk\n\
                  commons billing\n\n\
                  type Amount = Int where Positive\n\n\
                  // shipping.bynk\n\
                  commons shipping\n\n\
                  type Amount = Int where NonNegative    // both declare `Amount`\n\
                  type Postage = Int where NonNegative   // ✓ renamed\n\n\
                  // orders.bynk\n\
                  commons orders\n\n\
                  uses billing\n\
                  uses shipping                          // ✗ `Amount` is also declared by `billing`",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "",
    },
    Explain {
        code: "bynk.uses.self_reference",
        blurb: "`uses` brings another commons into scope. A commons' own declarations are already there, so using itself adds nothing, and a unit that depends on itself is a cycle of length one. Delete the clause, or, if you meant another commons, name it. The mirror of this for contexts is `bynk.consumes.self_reference`.",
        example: "// tax.bynk\ncommons tax\n\nfn rate() -> Int { 20 }\n\n// money.bynk\ncommons money\n\nuses money                       // ✗ a commons cannot use itself\nuses tax                         // ✓ a different commons\n\ntype Amount = Int where NonNegative",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "the-two-kinds-of-module",
    },
    Explain {
        code: "bynk.uses.target_is_context",
        blurb: "`uses` is for shared types and functions, and those live in a commons. A context is something to depend on: it has services to call and capabilities to use, and that is `consumes`. Replace `uses` with `consumes`. The mirror of this is `bynk.consumes.target_is_commons`.",
        example: "// pay.bynk\ncontext pay\n\n// shop.bynk\ncontext shop\n\nuses pay                         // ✗ `pay` is a context\nconsumes pay                     // ✓ depend on a context with `consumes`",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "the-two-kinds-of-module",
    },
    Explain {
        code: "bynk.uses.unknown_commons",
        blurb: "`uses` brings a commons' types, functions and constants into scope, so it must name a commons in the project, and no unit of that name exists. Check the spelling against the `commons` line of the unit you mean. If the unit is a context, `uses` is the wrong clause: that is `bynk.uses.target_is_context`.",
        example: "// money.bynk\ncommons money\n\ntype Amount = Int where NonNegative\n\n// shop.bynk\ncontext shop\n\nuses mony                        // ✗ no commons `mony`\nuses money                       // ✓ the commons' declared name",
        page: "/book/guides/program-structure/how-a-program-is-shaped",
        anchor: "the-two-kinds-of-module",
    },
    Explain {
        code: "bynk.val.agent_not_generable",
        blurb: "A `for all` over a declared type draws values from it, but an agent is not a value: a state that satisfies every invariant need not be one the agent can reach. So an agent cannot be a `for all` binding. To test an agent over generated input, bind a `History` of it, which drives real handler calls and so reaches only real states.",
        example: "// tests/t.test.bynk\nfor all w: Wallet { … }              // ✗ an agent is not generated\nfor all run: History[Wallet] { … }   // ✓ a run of its handlers",
        page: "/book/reference/testing",
        anchor: "properties",
    },
    Explain {
        code: "bynk.val.arity",
        blurb: "`Val[T]` takes at most one argument, the pin: `Val[Age]` lets the \
                compiler choose a value, `Val[Age](20)` fixes it. `Val` makes one \
                value of one type, so a second argument has no meaning. Pass one \
                pin or none.",
        example: "// demo/ages.bynk\n\
                  type Age = Int where InRange(0, 150)\n\
                  \n\
                  // tests/ages.test.bynk\n\
                  let a = Val[Age](20, 30)   // ✗ two pins\n\
                  let a = Val[Age](20)       // ✓ one",
        page: "/book/reference/testing",
        anchor: "val",
    },
    Explain {
        code: "bynk.val.generic_type",
        blurb: "`Val[T]` fabricates a value for a test from the declaration of `T`. \
                That is not yet done for each instantiation of a generic type, so \
                `Val` refuses one whether it is written bare, `Val[Box]`, or \
                applied, `Val[Box[Int]]`. The type itself is fine, and may cross a \
                boundary through its codec; only fabrication is missing. Build the \
                value yourself instead: a record literal for a generic record, \
                one of its variants for a generic sum.",
        example: "type Box[T] = { v: T }\n\nlet b = Val[Box[Int]]    // ✗ a generic type cannot be fabricated\n\nlet b = Box { v: 1 }     // ✓ build it",
        page: "/book/reference/testing",
        anchor: "val",
    },
    Explain {
        code: "bynk.val.literal_violates",
        blurb: "A pinned `Val[T](…)` is checked against the refinement of `T` at \
                compile time, and this literal breaks it, so the test would build \
                a value the type does not allow. Pin a value that satisfies the \
                refinement, or drop the pin and let `Val[T]` choose one. To test \
                that an invalid value is refused, call `T.of(…)` and expect an \
                `Err`.",
        example: "// demo/ages.bynk\n\
                  type Age = Int where InRange(0, 150)\n\
                  \n\
                  // tests/ages.test.bynk\n\
                  let a = Val[Age](200)  // ✗ 200 is outside InRange(0, 150)\n\
                  let a = Val[Age](20)   // ✓ inside it",
        page: "/book/reference/testing",
        anchor: "val",
    },
    Explain {
        code: "bynk.val.needs_pin",
        blurb: "In a test, a bare `Val[T]` fabricates a valid value of `T`. It can do that for \
                most refinements (`Positive` gives `1`, `MinLength(3)` a string of length three) \
                but not for a `Matches` pattern or `IsoDate`, so it refuses rather than guess; \
                the same goes for a record or sum that reaches one. A `for all` over such a type \
                is refused too. Pin a value, `Val[Code](\"abc\")`, which is checked against the \
                refinement, or give the type an `@example` and use bare `Val`. See the \
                `bynk.val.*` codes for pins that are not literals or do not satisfy the type.",
        example: "// demo/codes.bynk\n\
                  type Code = String where Matches(\"[a-z]+\")\n\
                  \n\
                  // tests/codes.test.bynk\n\
                  let c = Val[Code]              // ✗ no value can be made from a pattern\n\
                  let c = Val[Code](\"abc\")      // ✓ pin one",
        page: "/book/reference/testing",
        anchor: "val",
    },
    Explain {
        code: "bynk.val.outside_test",
        blurb: "`Val[T]` fabricates a value that merely satisfies `T`. That is \
                what a test wants and what running code must not do, so `Val` is \
                allowed only in test code. In a function or handler, take the \
                value as a parameter or build it properly, for instance with \
                `Age.of(20)`. See `bynk.val.needs_pin` for a bare `Val` that a \
                test cannot generate.",
        example: "// demo/ages.bynk\n\
                  type Age = Int where InRange(0, 150)\n\
                  \n\
                  fn sample() -> Age { Val[Age] }   // ✗ not test code\n\
                  \n\
                  // tests/ages.test.bynk\n\
                  let a = Val[Age]                  // ✓ in a case",
        page: "/book/reference/testing",
        anchor: "val",
    },
    Explain {
        code: "bynk.val.pin_not_literal",
        blurb: "A pin on `Val[T]` is checked against the refinement of `T` when \
                the program compiles, so it must be a literal of `T`'s base type. \
                A name or a call has no value at that point. Write the literal in \
                place. For a computed value, build it with `Age.of(…)` and `?`, \
                which fails the case if the value is not valid.",
        example: "// demo/ages.bynk\n\
                  type Age = Int where InRange(0, 150)\n\
                  \n\
                  // tests/ages.test.bynk\n\
                  let n = 20\n\
                  let a = Val[Age](n)    // ✗ a name, not a literal\n\
                  let a = Val[Age](20)   // ✓ a literal",
        page: "/book/reference/testing",
        anchor: "val",
    },
    Explain {
        code: "bynk.val.pin_unsupported",
        blurb: "A pinned `Val[T](lit)` is accepted only for a refined type, where the literal is checked against the refinement. A sum, record or opaque type takes no pin, so one is refused. Use bare `Val[T]`, which builds the first variant, or every field, itself, or construct the value directly with a variant or a record literal. A pin that is not a literal at all is `bynk.val.pin_not_literal`.",
        example: "// tests/demo.test.bynk\nlet s = Val[Status](1)   // ✗ `Status` is a sum\nlet s = Val[Status]      // ✓ bare `Val` picks the first variant",
        page: "/book/reference/testing",
        anchor: "val",
    },
    Explain {
        code: "bynk.val.unknown_type",
        blurb: "`Val[T]` fabricates a value of the type it names, so the name must \
                resolve to a type the suite can see. A misspelt name stops here, \
                before anything else about the value can be checked. Fix the \
                spelling, or declare the type in the target unit or the suite. The \
                same code covers a `for all` binder whose type does not resolve.",
        example: "// demo/ages.bynk\n\
                  type Age = Int where InRange(0, 150)\n\
                  \n\
                  // tests/ages.test.bynk\n\
                  let a = Val[Agee]      // ✗ no such type\n\
                  let a = Val[Age]       // ✓ the declared type",
        page: "/book/reference/testing",
        anchor: "val",
    },
    Explain {
        code: "bynk.val.unsupported_kind",
        blurb: "`Val[T]` fabricates a value from a type declaration: a refinement, a sum, a record or an opaque type. A built-in type such as `Int` or `String`, a `List` or a function type has no declaration to draw from, so `Val` refuses it. Write the value, `20`, or declare a type for it, which `Val` can then fabricate.",
        example: "type Age = Int where InRange(0, 150)\n\nlet n = Val[Int]   // ✗ a built-in type\nlet a = Val[Age]   // ✓ a declared type",
        page: "/book/reference/testing",
        anchor: "val",
    },
    Explain {
        code: "bynk.visibility.private_in_public",
        blurb: "A public declaration may not mention a `private` type of its own unit in its \
                signature: a public function's parameters and return, a public type's fields and \
                variant payloads, a public constant's type, or a context's `exports` list. A \
                unit that `uses` the commons could neither name that type nor build a value of \
                it, so the function would be half-usable. Make the type public, or make the \
                declaration `private` too. Services, agents and capabilities are not checked \
                here; a context's `exports` governs them.",
        example: "private type Tally = { lines: Int, sum: Int }\n\
                  \n\
                  fn tally(n: Int) -> Tally { … }            // ✗ public, but returns a private type\n\
                  private fn tally(n: Int) -> Tally { … }    // ✓ private too",
        page: "/book/guides/program-structure/design-a-public-surface",
        anchor: "keep-private-types-out-of-public-signatures",
    },
    Explain {
        code: "bynk.visibility.private_item",
        blurb: "A `private` function, type, constant, asset or embed (or a `private` method) is visible only \
                within its own unit, across all of that unit's files, and to the unit's suites. It is \
                never visible through `uses`, so a unit that uses the commons cannot name it. The \
                compiler reports that the name is private, not that it is unknown, so you know it \
                exists. `private` keeps a helper out of the public surface, so it can change \
                without breaking anyone. Call the commons' public function that does the job, or, \
                if the item really belongs to the public surface, remove `private` where it is \
                declared.",
        example: "// pricing.bynk\n\
                  commons pricing\n\n\
                  fn total(items: List[Item]) -> Int { … }\n\
                  private fn lineTotal(i: Item) -> Int { … }\n\n\
                  // shop.bynk\n\
                  commons shop\n\n\
                  uses pricing\n\n\
                  fn cost(i: Item) -> Int {\n    lineTotal(i)      // ✗ `lineTotal` is a private function of `pricing`\n    total([i])        // ✓ the public function does the same job\n}",
        page: "/book/guides/program-structure/design-a-public-surface",
        anchor: "",
    },
    Explain {
        code: "bynk.visibility.private_not_allowed",
        blurb: "`private` hides a `fn`, `type`, `messages`, `asset`, `embed` or `const` from the \
                units that import its commons or context. It does not apply to a service, agent, \
                capability, provider, actor or event, or to anything in an adapter: how far \
                those reach is decided by a context's `exports` and `consumes`, and an adapter's \
                `exports`. Remove `private`. See `bynk.visibility.private_item` for using \
                something that is private.",
        example: "private service total { … }        // ✗ `private` does not apply to a service\n\
                  service total { … }                // ✓ reach is governed by `exports`",
        page: "/book/guides/program-structure/design-a-public-surface",
        anchor: "",
    },
    Explain {
        code: "bynk.within.in_agent_handler",
        blurb: "`within` hands the effect it abandons on a timeout to the execution context, and an agent handler has none. Call it from the service handler that calls the agent, so the agent's work is bounded from outside. For a slow call out, `FetchOptions.timeout` bounds it inside the agent. The same rule holds for `bynk.race.in_agent_handler`.",
        example: "agent Feed {\n    key id: String\n\n    on call quote() -> Effect[Result[Int, TimedOut]] {\n        Slow(\"a\").get().within(1.seconds)   // ✗ an agent handler has no execution context\n    }\n}\n\nservice quotes {\n    on call() -> Effect[Result[Int, TimedOut]] {\n        Slow(\"a\").get().within(1.seconds)   // ✓ the service handler has\n    }\n}",
        page: "/book/reference/capabilities",
        anchor: "within",
    },
    Explain {
        code: "bynk.within.in_pure_context",
        blurb: "`within` hands the effect it abandons on a timeout to the execution context, and only an effectful body has one: a pure function never runs an effect at all. Change the enclosing function's return type to `Effect[…]` so its body can run it. The same rule holds for `bynk.race.in_pure_context` and `bynk.send.in_pure_context`.",
        example: "fn bounded(e: Effect[Int]) -> Int {\n    let r = e.within(1.seconds)   // ✗ the body is pure\n    0\n}\n\nfn bounded(e: Effect[Int]) -> Effect[Result[Int, TimedOut]] {   // ✓ an effectful body\n    e.within(1.seconds)\n}",
        page: "/book/reference/capabilities",
        anchor: "within",
    },
    Explain {
        code: "bynk.within.outside_handler",
        blurb: "`within` hands the effect it abandons on a timeout to the execution context, which only a service handler's body, or a function's it calls, has. A WebSocket handler, a provider operation and a test `case` have none. Move the `within` into a service handler or a function, and have the case call that, simulating a slow call with `stub … after`. In an agent handler the rule is `bynk.within.in_agent_handler`.",
        example: "// tests/t.test.bynk\nlet r <- Slow(\"a\").get().within(1.seconds)   // ✗ a case has no execution context\n\nlet r <- quotes.call()                      // ✓ the service handler bounds it",
        page: "/book/reference/capabilities",
        anchor: "within",
    },
    Explain {
        code: "bynk.within.timed_out_not_in_scope",
        blurb: "`within` returns `Result[_, TimedOut]`, and `TimedOut` is declared in `bynk`, so a unit that wants `within` has to consume it. Without that the result type names a type the unit cannot see. Add `consumes bynk { TimedOut }` to the unit, or add `TimedOut` to its existing `consumes bynk { … }` list.",
        example: "let r <- Pricing.quote(id).within(2.seconds)   // ✗ `TimedOut` is not in scope\n\nconsumes bynk { TimedOut }                     // ✓ in the unit's header\nlet r <- Pricing.quote(id).within(2.seconds)",
        page: "/book/reference/capabilities",
        anchor: "within",
    },
    Explain {
        code: "bynk.ws.lifecycle_by_mismatch",
        blurb: "The connection's principal is verified once, by `on open`'s `by`, and carried for the life of the connection; `on message` and `on close` read it and never verify again. So their `by` must name the same actor as `on open`'s, or the same members of an actor sum. The binder names may differ. Change the lifecycle handler's `by` to match `on open`.",
        example: "actor Member { … }\nactor Admin { … }\n\nservice gateway from websocket(in: Frame, out: Frame) {\n    on open (room: RoomId) -> Effect[()] by user: Member { … }\n    on message (room: RoomId, frame: Frame) -> Effect[()] by admin: Admin { … }   // ✗ `on open` is `by` `Member`\n    on message (room: RoomId, frame: Frame) -> Effect[()] by user: Member { … }   // ✓\n}",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
    Explain {
        code: "bynk.ws.message_frame_param",
        blurb: "A WebSocket `on message` handler is called with a decoded inbound frame, so it needs exactly one parameter of the service's `in:` type. Any other parameters are route values recovered from the connection. Add the frame parameter. A route value whose type differs from `on open`'s is `bynk.ws.route_param_mismatch`.",
        example: "service gateway from websocket(in: Frame, out: Frame) {\n    on message (room: RoomId) -> Effect[()] by user: Member { … }                  // ✗ no `Frame` parameter\n    on message (room: RoomId, frame: Frame) -> Effect[()] by user: Member { … }   // ✓\n}",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
    Explain {
        code: "bynk.ws.open_given_unsupported",
        blurb: "A WebSocket `on open` cannot declare `given` capabilities. It only accepts the upgrade and hands the connection to an agent; on Workers it runs inside the Durable Object that holds the connection, where there is no composition root to supply capabilities. Put the work that needs a capability in the agent handler the connection is transferred to, which carries its own `given`.",
        example: "service gateway from websocket(in: Frame, out: Frame) {\n    on open (room: RoomId) -> Effect[()] by user: Member given Clock { … }   // ✗ no capabilities on `on open`\n}\n\nservice gateway from websocket(in: Frame, out: Frame) {\n    on open (room: RoomId) -> Effect[()] by user: Member { … }   // ✓ the agent's handler takes the `given`\n}",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
    Explain {
        code: "bynk.ws.open_transfer_shape",
        blurb: "A WebSocket `on open` must hand its `connection` to exactly one agent, unconditionally, as `Room(room).join(user.identity, connection)`. On Workers the upgrade is routed to that agent's Durable Object, which holds the connection, so it needs a single target the compiler can find. Neither none nor several will do, and a transfer inside an `if` or a `match` is not found. A connection left undisposed is `bynk.held.leak`; a transfer whose key the edge cannot compute is `bynk.ws.open_transfer_key`.",
        example: "service gateway from websocket(in: Frame, out: Frame) {\n    on open (room: RoomId) -> Effect[()] by user: Member {   // ✗ `connection` is never transferred\n        let _ <- connection.close()\n        ()\n    }\n}\n\nservice gateway from websocket(in: Frame, out: Frame) {\n    on open (room: RoomId) -> Effect[()] by user: Member {\n        let _ <- Room(room).join(user.identity, connection)   // ✓ handed to one agent\n        ()\n    }\n}",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
    Explain {
        code: "bynk.ws.route_param_mismatch",
        blurb: "`on message` and `on close` are not given a fresh URL: their route values are recovered, in order, from what `on open` accepted. So the leading parameters of each must have the same types as `on open`'s, in the same order. A different type at some position would reinterpret one route value as another. Match the types of `on open`'s parameters; the names may differ. A missing frame parameter is `bynk.ws.message_frame_param`.",
        example: "on open (room: RoomId) -> Effect[()] by user: Member { … }\non close (who: UserId) -> Effect[()] by user: Member { … }   // ✗ the first route value is a `RoomId`\n\non open (room: RoomId) -> Effect[()] by user: Member { … }\non close (room: RoomId) -> Effect[()] by user: Member { … }   // ✓ same leading types",
        page: "/book/reference/websocket",
        anchor: "service-form",
    },
];

/// The curated explanation for a diagnostic `code`, or `None` when the code has
/// no explanation yet (the designed graceful-fallback state, DECISION B).
pub fn explain(code: &str) -> Option<&'static Explain> {
    EXPLANATIONS.iter().find(|e| e.code == code)
}

/// The hosted diagnostic index page (the generated `reference/diagnostics`
/// page), the link target for a code with no curated [`Explain`] page.
pub const INDEX_PATH: &str = "/book/reference/diagnostics/";

/// The anchor slug of a category's `## <title>` heading on the generated
/// diagnostic index (Starlight/github-slugger shape: lowercase, spaces to
/// hyphens, punctuation dropped), e.g. `"type-checking"`.
fn category_anchor(cat: &str) -> String {
    category_title(cat)
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// The hosted-Book link for any diagnostic `code`: the curated
/// [`Explain::href`] when one exists (what the LSP's `codeDescription` uses),
/// otherwise the diagnostic index, deep-linked to the code's category section
/// when the code is a registry code (a non-registry code, e.g. a host-synthetic
/// one, links the index top).
pub fn href(code: &str) -> String {
    if let Some(e) = explain(code) {
        return e.href();
    }
    let mut url = format!("{BOOK_BASE_URL}{INDEX_PATH}");
    if lookup(code).is_some() {
        url.push('#');
        url.push_str(&category_anchor(category(code)));
    }
    url
}

/// Look up a code's registry entry by exact match — the one place
/// [`crate::error::Severity::for_error`] reads a code's severity, instead of
/// hardcoding its own copy of the (small) warning set.
pub fn lookup(code: &str) -> Option<&'static DiagnosticInfo> {
    REGISTRY.iter().find(|d| d.code == code)
}

/// Every diagnostic code the compiler emits, sorted by code.
pub const REGISTRY: &[DiagnosticInfo] = &[
    d(
        "bynk.actor.bearer_identity_not_string_constructible",
        "A `Bearer` actor's identity is not a string-constructible type.",
    ),
    d(
        "bynk.actor.bearer_missing_secret",
        "A `Bearer` actor does not name its signing secret.",
    ),
    d(
        "bynk.actor.binder_shadows_param",
        "A `by` actor binder collides with a handler parameter of the same name.",
    ),
    d(
        "bynk.actor.by_on_agent",
        "A `by` actor clause was placed on an agent `on call` handler, which has no actor.",
    ),
    d(
        "bynk.actor.duplicate_sum_scheme",
        "Two peers in a multi-actor sum share an authentication scheme.",
    ),
    d(
        "bynk.actor.foreign_context_name_unkeyable",
        "A context declaring a `Foreign` actor has a name no call key can be named from.",
    ),
    d(
        "bynk.actor.foreign_duplicate_secret",
        "A `Foreign` actor's secret is also another actor's secret in the same context.",
    ),
    d(
        "bynk.actor.foreign_identity_unsupported",
        "A `Foreign` actor declares an `identity`.",
    ),
    d(
        "bynk.actor.foreign_missing_secret",
        "A `Foreign` actor does not name the secret its calls are signed with.",
    ),
    d(
        "bynk.actor.foreign_name_is_context_name",
        "A `Foreign` actor's name could be read as a context's name.",
    ),
    d(
        "bynk.actor.foreign_reserved_secret",
        "A `Foreign` actor's secret uses a `BYNK_CALL_` edge key's name.",
    ),
    d(
        "bynk.actor.foreign_secret_not_string",
        "A `Foreign` actor's secret is not a string.",
    ),
    d(
        "bynk.actor.identity_not_sealed",
        "An actor identity type is not a context-ownable (sealed) value type.",
    ),
    d(
        "bynk.actor.missing_by_on_http",
        "An HTTP handler lacks the required `by` actor clause.",
    ),
    d(
        "bynk.actor.oidc_identity_not_string_constructible",
        "An `Oidc` actor's identity is not a string-constructible type.",
    ),
    d(
        "bynk.actor.oidc_missing_audience",
        "An `Oidc` actor does not name its `audience`.",
    ),
    d(
        "bynk.actor.oidc_missing_issuer",
        "An `Oidc` actor does not name its `issuer`.",
    ),
    d(
        "bynk.actor.oidc_missing_jwks",
        "An `Oidc` actor does not name its `jwks` endpoint.",
    ),
    d(
        "bynk.actor.oidc_not_in_sum",
        "An `Oidc` actor appears as a member of a multi-actor sum.",
    ),
    d(
        "bynk.actor.oidc_trust_from_is_secret",
        "An `Oidc` actor's `trustFrom` names an environment variable that an actor uses as its `secret`.",
    ),
    d(
        "bynk.actor.oidc_trust_from_not_string",
        "An `Oidc` actor's `trustFrom` isn't a string naming an environment variable.",
    ),
    d(
        "bynk.actor.outside_context",
        "An `actor` was declared outside a context (e.g. in a commons).",
    ),
    d(
        "bynk.actor.refinement_base_unsupported",
        "A refinement actor's base is not a `Bearer` actor (no claims to authorise against).",
    ),
    d(
        "bynk.actor.refinement_in_sum",
        "A refinement actor appears as a member of a multi-actor sum.",
    ),
    d(
        "bynk.actor.refinement_predicate_unsupported",
        "A refinement actor's `where` predicate is outside the closed claim-predicate set.",
    ),
    d(
        "bynk.actor.scheme_arg_kind",
        "A scheme config value other than `SignedCookie`'s is a duration or a bare name, which only `SignedCookie` takes.",
    ),
    d(
        "bynk.actor.scheme_not_admissible",
        "An actor's scheme is not admissible on this handler's protocol.",
    ),
    d(
        "bynk.actor.signature_identity_unsupported",
        "A `Signature` actor declared an `identity`, which is not yet supported.",
    ),
    d(
        "bynk.actor.signature_missing_header",
        "A `Signature` actor does not name its signature header.",
    ),
    d(
        "bynk.actor.signature_missing_secret",
        "A `Signature` actor does not name its signing secret.",
    ),
    d(
        "bynk.actor.signature_requires_body",
        "A `Signature` handler does not take a `body` parameter.",
    ),
    d(
        "bynk.actor.signature_tolerance_without_timestamp",
        "A `Signature` actor set `tolerance` without a `timestamp` header.",
    ),
    d(
        "bynk.actor.signed_cookie_bad_arg",
        "A `SignedCookie` config value has the wrong kind.",
    ),
    d(
        "bynk.actor.signed_cookie_bad_name",
        "A `SignedCookie` `cookie` is not a legal cookie name.",
    ),
    d(
        "bynk.actor.signed_cookie_bad_ttl",
        "A `SignedCookie` `ttl` is not a positive whole number of seconds.",
    ),
    d(
        "bynk.actor.signed_cookie_duplicate_key",
        "A `SignedCookie` config repeats a key.",
    ),
    d(
        "bynk.actor.signed_cookie_identity_not_string_constructible",
        "A `SignedCookie` actor's identity is missing or not a string-constructible type.",
    ),
    d(
        "bynk.actor.signed_cookie_issue_needs_clock",
        "`Actor.issue(identity)` on a `SignedCookie` actor is called where the handler does not list `Clock` in `given`.",
    ),
    d(
        "bynk.actor.signed_cookie_issue_outside_handler",
        "`Actor.issue(identity)` on a `SignedCookie` actor is called outside a service handler (in a provider or an agent).",
    ),
    d(
        "bynk.actor.signed_cookie_missing_secret",
        "A `SignedCookie` actor does not name its signing secret.",
    ),
    d(
        "bynk.actor.signed_cookie_missing_ttl",
        "A `SignedCookie` actor does not declare a `ttl`.",
    ),
    d(
        "bynk.actor.signed_cookie_not_in_sum",
        "A `SignedCookie` actor appears as a member of a multi-actor sum.",
    ),
    d(
        "bynk.actor.signed_cookie_public_cache",
        "A `@cache(scope: public)` route is guarded by a `SignedCookie` actor.",
    ),
    d(
        "bynk.actor.signed_cookie_read_directly",
        "A typed `cookies` field reads the session cookie that a route's `SignedCookie` actor consumes.",
    ),
    d(
        "bynk.actor.signed_cookie_secret_shared",
        "A `SignedCookie` actor's `secret` or `previous` is also used by another actor, or by itself.",
    ),
    warn(d(
        "bynk.actor.signed_cookie_ttl_long",
        "A `SignedCookie` `ttl` is longer than 30 days (warning).",
    )),
    d(
        "bynk.actor.signed_cookie_unknown_key",
        "A `SignedCookie` config has a key outside the closed set.",
    ),
    warn(d(
        "bynk.actor.signed_cookie_unprefixed",
        "A `SignedCookie` `cookie` does not start with `__Host-` (warning).",
    )),
    d(
        "bynk.actor.signed_cookie_write_directly",
        "A cookie record's typed write names the session cookie that a `SignedCookie` actor consumes.",
    ),
    d(
        "bynk.actor.signed_cookie_write_literal",
        "An untyped `Cookie` literal is named like a `SignedCookie` actor's session cookie.",
    ),
    d(
        "bynk.actor.sum_requires_binder",
        "A multi-actor sum `by` clause has no binder to match the resolved actor.",
    ),
    d(
        "bynk.actor.unknown_actor",
        "A handler's `by` clause names an actor that is not declared.",
    ),
    d(
        "bynk.actor.unknown_scheme",
        "An actor declares an authentication scheme that is not compiler-known.",
    ),
    d(
        "bynk.actor.unreachable_sum_arm",
        "A multi-actor sum has an arm unreachable after a catch-all (`None`) peer.",
    ),
    dg(
        "bynk.adapter.consumes_context",
        "An `adapter` consumed a context; adapter dependencies are adapter-to-adapter.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.adapter.consumes_requires_selection",
        "An `adapter` used a whole-unit or aliased `consumes`; adapters must select capabilities with `consumes U { Cap, … }`.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.adapter.disallowed_item",
        "An `adapter` declared a `service`, `agent`, or other item it may not contain.",
        &["adapter_decl"],
    ),
    dg(
        "bynk.adapter.duplicate_binding",
        "An `adapter` declared more than one `binding` clause.",
        &["binding_decl"],
    ),
    dg(
        "bynk.adapter.include_duplicate",
        "An adapter `binding … includes [ … ]` list names one file more than once.",
        &["binding_decl"],
    ),
    dg(
        "bynk.adapter.include_kind",
        "An adapter `binding … includes [ … ]` entry is not a `.wasm` file; only WebAssembly modules can be included.",
        &["binding_decl"],
    ),
    dg(
        "bynk.adapter.include_missing",
        "An adapter `binding … includes [ … ]` entry names a file that could not be read.",
        &["binding_decl"],
    ),
    dg(
        "bynk.adapter.include_outside",
        "An adapter `binding … includes [ … ]` path is absolute or climbs out of the adapter's directory with `..`.",
        &["binding_decl"],
    ),
    dg(
        "bynk.adapter.no_binding",
        "An `adapter` declares an external provider but no `binding` module to supply it.",
        &["adapter_decl"],
    ),
    dg(
        "bynk.adapter.provider_has_body",
        "A provider inside an `adapter` has a Bynk body; adapter providers must be external.",
        &["provider_decl"],
    ),
    d(
        "bynk.agent.class_conflict",
        "An agent class has more than one fate: it is renamed twice, both renamed and removed, or renamed onto a class that already has its own recorded history in `bynk.schema.lock`.",
    ),
    d(
        "bynk.agent.class_removed",
        "An agent class recorded in `bynk.schema.lock` is no longer declared, so a deployment's stored instances of it would be orphaned. If the class was never deployed, delete its `[agents]` entry from the lock; renaming or removing a deployed class gets its own declaration in a later state-migrations slice.",
    ),
    dg(
        "bynk.agent.construction_arity",
        "An agent was constructed with the wrong number of key arguments.",
        &["agent_decl"],
    ),
    dg(
        "bynk.agent.handler_arity",
        "An agent handler was called with the wrong number of arguments.",
        &["agent_decl"],
    ),
    dg(
        "bynk.agent.handler_not_found",
        "Called a handler the agent does not declare.",
        &["agent_decl"],
    ),
    dg(
        "bynk.agent.key_mismatch",
        "An agent key argument has the wrong type.",
        &["agent_decl"],
    ),
    warn(d(
        "bynk.agent.migrate_below_floor",
        "A `migrate` step reads a schema version below the agent's floor. A complete `bynk migrate sweep` raised the floor past it, so no stored instance can hold that version: the step emits nothing and can be deleted (warning).",
    )),
    d(
        "bynk.agent.migrate_duplicate",
        "Two `migrate` steps in one agent read the same schema version.",
    ),
    d(
        "bynk.agent.migrate_duplicate_assignment",
        "A `migrate` step assigns the same `store` field more than once; each field takes at most one value per step (record-literal semantics).",
    ),
    d(
        "bynk.agent.migrate_impure",
        "A `migrate` step uses an effect, a capability, `?`, `expect`, or the agent's `key`; steps are pure and key-free so a migration sweep can run them by object ID.",
    ),
    d(
        "bynk.agent.migrate_incomplete",
        "A `migrate` step does not account for every change between its version and the current shape: a removed field must be read (`old.f`) or dropped, and a retyped or re-kinded field must be assigned (or dropped, for a `Set`, `Log` or `Cache` field).",
    ),
    d(
        "bynk.agent.migrate_shape_mismatch",
        "A `migrate` step's declared old shape (or a `drop`) names a field the lock does not record at that version, or records with a different structure.",
    ),
    d(
        "bynk.agent.migrate_unknown_version",
        "A `migrate` step reads a schema version the lock does not record below the agent's current version.",
    ),
    d(
        "bynk.agent.non_additive_state_change",
        "An agent's stored state changed in a way deployed data cannot be read through — a `store` field was removed, retyped (including inside a stored record or sum), or changed storage kind. A renamed field is a removed field plus an added one. If the class was never deployed, delete its `[agents]` entry from `bynk.schema.lock`; otherwise widen rather than narrow, or stage the change through a new field.",
    ),
    dg(
        "bynk.agent.outside_context",
        "An `agent` was declared outside a context.",
        &["agent_decl"],
    ),
    warn(d(
        "bynk.agent.refinement_changed",
        "A stored `store` field's refinement changed while its structure did not. A widening is safe; a narrowing faults on load for any stored value that no longer satisfies it (`RehydrationViolation`), so migrate the data first (warning).",
    )),
    d(
        "bynk.agent.removed_live",
        "A `removed agent` declaration names a class that is still declared; removing a class deletes its stored data.",
    ),
    d(
        "bynk.agent.replaces_live",
        "`agent New replaces Old` names an `Old` that is still declared; a rename moves the old class's data to the new one.",
    ),
    d(
        "bynk.agent.replaces_unknown",
        "`agent New replaces Old` names an `Old` that `bynk.schema.lock` does not record for this context, so there is no recorded class to rename.",
    ),
    dg(
        "bynk.agent.return_not_effect",
        "An agent handler's return type is not an `Effect`.",
        &["agent_decl"],
    ),
    d(
        "bynk.agent.state_name_conflict",
        "A type is named `<Agent>State` beside agent `<Agent>`; that name is the agent's state record (its `Cell` fields), which its handlers and invariants see in place of the type.",
    ),
    warn(d(
        "bynk.agent.version_without_path",
        "A schema version between the agent's floor and its current version has no way forward: no `migrate` step covers it and its path to the current shape is not entirely additive, so a stored record at that version faults on load with `NoMigrationPath` (warning).",
    )),
    dg(
        "bynk.agents.bad_state_initialiser",
        "An agent `store` field initialiser is not a static value of the field's type.",
        &["store_field"],
    ),
    dg(
        "bynk.agents.non_zeroable_state_field",
        "An agent `store` field has no initialiser and no implicit zero value.",
        &["store_field"],
    ),
    d(
        "bynk.asset.duplicate",
        "An `asset` name is declared more than once in the same unit.",
    ),
    d(
        "bynk.asset.invalid_at_path",
        "An `asset`'s `at` path is not a URL path naming one file, or lies under `/_assets/` or `/_bynk/`, or is a file the platform reads as configuration.",
    ),
    warn(d(
        "bynk.asset.may_shadow_route",
        "A parameterised HTTP route matches a fixed asset path, so it never sees requests for that path (a warning).",
    )),
    d(
        "bynk.asset.missing_file",
        "An `asset` names a file that is not in the project's `assets/` folder.",
    ),
    d(
        "bynk.asset.needs_project",
        "A single-file build met an `asset` declaration; assets need a project build.",
    ),
    warn(d(
        "bynk.asset.not_served",
        "A deployable Bundle build met an `asset`; a Bundle ships no files, so its `.url` would not be served (a warning).",
    )),
    d(
        "bynk.asset.outside_assets_dir",
        "An `asset` path does not name a file in the project's `assets/` folder.",
    ),
    d(
        "bynk.asset.outside_context_or_commons",
        "An `asset` declaration appears outside a context or commons.",
    ),
    d(
        "bynk.asset.path_conflict",
        "Two different asset files would be served at one fixed path by the same Worker.",
    ),
    d(
        "bynk.asset.shadows_route",
        "A fixed asset path equals a literal HTTP route, which the platform-served asset makes unreachable.",
    ),
    d(
        "bynk.asset.unreadable_file",
        "An `asset` names a file in the project's `assets/` folder that could not be read.",
    ),
    d(
        "bynk.asset.unsafe_path",
        "An `asset` path is empty, absolute, or contains a `..` segment.",
    ),
    d(
        "bynk.boundary.ambiguous_type",
        "A value's type is declared, with different shapes, by more than one consumed context, so its fields cannot be read.",
    ),
    d(
        "bynk.boundary.structural_mismatch",
        "Data crossing a context boundary did not match the expected shape.",
    ),
    dg(
        "bynk.capability.base_not_instantiable",
        "A derived capability's base is not a platform resource that can be instantiated.",
        &["capability_decl"],
    ),
    dg(
        "bynk.capability.derived_outside_context",
        "A derived capability was declared outside a context.",
        &["capability_decl"],
    ),
    dg(
        "bynk.capability.instance_only",
        "A platform capability that is used only through named instances (`Service`, `R2`, `RateLimit`, `Queue`, `Analytics`, `D1`, `EdgeCache`) was used bare, in a `given` clause.",
        &["given_clause"],
    ),
    dg(
        "bynk.capability.invalid_option",
        "A derived capability's option is not a single positional literal of its kind (string, positive `Int`, or an offered window), is given twice, or is a value the kind refuses (an `@worker` that is not a well-formed Worker name outside the project, a malformed queue `@name`, an unknown `@jurisdiction`).",
        &["capability_decl"],
    ),
    dg(
        "bynk.capability.missing_option",
        "A derived capability lacks an option its base requires, such as a `Service` instance's `@worker`.",
        &["capability_decl"],
    ),
    dg(
        "bynk.capability.not_provided",
        "On the Workers target, a capability a context requires has no provider in it.",
        &["given_clause"],
    ),
    dg(
        "bynk.capability.op_arity",
        "A capability operation was called with the wrong number of arguments.",
        &["capability_decl"],
    ),
    dg(
        "bynk.capability.outside_context",
        "A `capability` was declared outside a context.",
        &["capability_decl"],
    ),
    dg(
        "bynk.capability.provider_agent_cross_context",
        "On the Workers target, a context requires a capability whose provider, in another context, constructs that context's agents; the consumer's Worker would build the provider without a binding for those agents' Durable Objects.",
        &["given_clause"],
    ),
    dg(
        "bynk.capability.unknown_base",
        "A derived capability's base is not a capability the context consumes.",
        &["capability_decl"],
    ),
    dg(
        "bynk.capability.unknown_operation",
        "Referenced an operation the capability does not declare.",
        &["capability_decl"],
    ),
    dg(
        "bynk.capability.unknown_option",
        "A derived capability carries an option its base does not take.",
        &["capability_decl"],
    ),
    d(
        "bynk.cell.invalid_target",
        "A `:=` write targets something that is not a `store Cell` field.",
    ),
    d(
        "bynk.cell.self_reference",
        "A `:=` right-hand side reads the cell being written (a read-modify-write); use `.update`.",
    ),
    d(
        "bynk.cloudflare.edge_cache_duplicate",
        "A context declares a second `EdgeCache` instance; the edge cache is one setting for the whole Worker.",
    ),
    d(
        "bynk.cloudflare.edge_cache_exported",
        "An exported capability reaches an `EdgeCache` instance through its provider's `given`; a consuming context would build it in its own Worker, whose purge misses this one's cache.",
    ),
    d(
        "bynk.cloudflare.edge_cache_in_agent",
        "An agent handler reaches an `EdgeCache` instance, directly or through a provider's `given`; an agent's purge would not reach the Worker's cache.",
    ),
    d(
        "bynk.cloudflare.edge_cache_in_queue",
        "A queue consumer reaches an `EdgeCache` instance; purge from a service handler instead.",
    ),
    d(
        "bynk.cloudflare.edge_cache_in_schedule",
        "An `on schedule` handler reaches an `EdgeCache` instance; a cron run's purge is accepted but clears nothing from the fetch cache, so purge from a service handler.",
    ),
    d(
        "bynk.cloudflare.edge_cache_public_authenticated",
        "In a context with `EdgeCache`, a `@cache(scope: public)` route is served by an actor that authenticates; its response would be shared at the edge with every caller.",
    ),
    d(
        "bynk.codegen.tsc_failed",
        "`tsc` rejected the TypeScript the compiler generated for a program that passed the checker. This is a bug in the Bynk compiler, not in the program: the diagnostic points at the Bynk code the failing generated line came from, with the TypeScript error attached (#3068).",
    ),
    dg(
        "bynk.consumes.alias_conflict",
        "Two `consumes` aliases collide.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.capability_name_clash",
        "Two flattened `consumes U { Cap }` capabilities collide, or one clashes with a local capability.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.name_conflict",
        "A `consumes` name collides with another name in scope.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.self_reference",
        "A context `consumes` itself.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.service_arity",
        "A consumed service was called with the wrong number of arguments.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.signature_type_unconsumed",
        "A cross-context call reaches a service whose signature names a type the consumed context itself consumes from a third context, which the caller does not consume.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.target_is_commons",
        "`consumes` targets a `commons` instead of a context.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.type_alias_capability",
        "A capability was aliased in `consumes U { Cap as Alias }`; only exported types can be aliased.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.type_alias_conflict",
        "A `consumes U { T as Alias }` alias collides with a type or value in scope, or with another alias.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.type_alias_repeated",
        "An exported type is aliased more than once, or is selected both bare and aliased, in `consumes U { … }`.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.type_alias_unknown_export",
        "`consumes U { T as Alias }` names a type `U` does not export.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.unknown_context",
        "`consumes` names a context that does not exist.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.consumes.unknown_service",
        "Called a service the consumed context does not declare.",
        &["consumes_decl"],
    ),
    d(
        "bynk.context.consumes_cycle",
        "Contexts form a `consumes` dependency cycle.",
    ),
    d(
        "bynk.context.duplicate_binding",
        "A Worker would declare two bindings with the same name, such as an agent `Kv` and the `bynk.cloudflare` KV namespace (both `KV`).",
    ),
    d(
        "bynk.context.external_construction",
        "A context-owned type was constructed from outside that context.",
    ),
    d(
        "bynk.context.external_decode",
        "A context decoded a type another context owns and exports opaque: through `Json.decode`, `Json.decodeSql` or a `Codable` witness, or as the input of an HTTP, queue, event or WebSocket handler. Decoding creates a value, which only the owning context may do; its codec would check no more than the base type.",
    ),
    dg(
        "bynk.context.external_provider",
        "A bodiless (external) provider was declared outside an `adapter`.",
        &["provider_decl"],
    ),
    d(
        "bynk.context.opaque_inspection",
        "An opaquely-exported type was inspected from outside its context.",
    ),
    d(
        "bynk.context.rebrand_construction",
        "A variant of a `uses`-sourced commons sum type was constructed directly inside a context, where the emitter's per-context rebrand leaves its constructors out of scope.",
    ),
    d(
        "bynk.contract.duplicate_name",
        "A function declares two contract clauses (`requires`/`ensures`) with the same name.",
    ),
    d(
        "bynk.contract.impure_predicate",
        "A contract predicate uses an effectful or test-only construct; a contract clause must be pure.",
    ),
    d(
        "bynk.contract.not_bool",
        "A contract predicate does not have type `Bool`.",
    ),
    d(
        "bynk.contract.restated_by_test",
        "A `case`/`property` merely restates a contract clause already declared at the function; the test is redundant.",
    ),
    d(
        "bynk.contract.result_in_requires",
        "A precondition (`requires`) references `result`; the return value is only in scope inside an `ensures`.",
    ),
    dg(
        "bynk.cron.bad_params",
        "A cron handler declares more than one parameter, or a non-`Int` one.",
        &["cron_handler"],
    ),
    dg(
        "bynk.cron.duplicate_schedule",
        "Two cron handlers declare the same schedule.",
        &["cron_handler"],
    ),
    dg(
        "bynk.cron.invalid_schedule",
        "A cron expression is not five whitespace-separated fields.",
        &["cron_handler"],
    ),
    dg(
        "bynk.cron.return_not_effect_result",
        "A cron handler does not return `Effect[Result[(), E]]`.",
        &["cron_handler"],
    ),
    warn(d(
        "bynk.d1.migration_invalid",
        "A `D1` migration does not apply to an empty in-memory SQLite, so the binding's literal SQL is not checked against its schema (warning).",
    )),
    d(
        "bynk.d1.migration_name",
        "A file in the project's `migrations/` folder isn't `migrations/<database>/NNNN_<name>.sql`, or shares its number with another migration of the same database.",
    ),
    d(
        "bynk.d1.migration_unreadable",
        "A `D1` migration in the project's `migrations/` folder can't be read, or isn't UTF-8 text.",
    ),
    warn(d(
        "bynk.d1.migrations_unused",
        "A directory in the project's `migrations/` folder matches no `D1` instance's database name, so its migrations are never applied, usually because the capability was renamed (warning).",
    )),
    d(
        "bynk.d1.select_field_missing",
        "A record read from a literal D1 statement has a field with no column of that name. For a `select[T]` / `firstAs[T]` row type, the column list the compiler writes from `T` does not prepare against the schema the binding's migrations build; for a `Json.decodeSql[T]` of the rows of a literal `all` / `first`, a required (non-`Option`) field of `T` has no result column.",
    ),
    d(
        "bynk.d1.sql_invalid",
        "A literal D1 statement does not prepare against the schema the binding's migrations build: a syntax error, or an unknown table or column.",
    ),
    d(
        "bynk.d1.sql_param_count",
        "A literal D1 statement uses a different number of bound parameters (`?N`) than the list literal passed with it holds.",
    ),
    d(
        "bynk.dependency.path_outside_root",
        "A path dependency declared by a dependency resolves outside the workspace; a dependency's own path dependencies are honoured only between members of one workspace.",
    ),
    d(
        "bynk.deploy.contract_skew",
        "A worker was compiled against a contract its live dependencies no longer provide, or no longer provides the contract its live callers were compiled against, so `bynk deploy --context` refuses to ship it.",
    ),
    d(
        "bynk.deploy.foreign_contract_skew",
        "A build breaks the contract a declared foreign caller's client was generated at, as pinned in `bynk.foreign.toml`, so `bynk deploy` refuses to ship it.",
    ),
    dg(
        "bynk.detach.in_agent_handler",
        "A `detach` was used in an agent handler, which has no execution context to keep a detached effect alive.",
        &["method_call"],
    ),
    dg(
        "bynk.detach.in_pure_context",
        "A `detach` was used in a pure (non-effectful) context.",
        &["method_call"],
    ),
    dg(
        "bynk.detach.store_op",
        "A `detach` targets an agent store operation, which is part of the handler's atomic commit and cannot be detached.",
        &["method_call"],
    ),
    // LSP-only (#2459): published by `bynk-lsp` as a hint (or warning, per the
    // `bynk.docLinks.reportUnresolved` setting); the compiler never emits it.
    warn(d(
        "bynk.doc.unresolved_link",
        "A `[Name]` doc link resolves to nothing in the unit's scope (editor hint only).",
    )),
    d(
        "bynk.duration.literal_overflow",
        "A `Duration` literal (`<int>.<unit>`) exceeds the representable millisecond range.",
    ),
    dg(
        "bynk.effect.bind_in_pure_context",
        "An `<-` bind was used in a pure (non-effectful) context.",
        &["effect_let_stmt"],
    ),
    dg(
        "bynk.effect.bind_on_non_effect",
        "An `<-` bind was applied to a non-`Effect` value.",
        &["effect_let_stmt"],
    ),
    d(
        "bynk.effect.capability_in_pure_context",
        "A capability was used in a pure context.",
    ),
    d(
        "bynk.effect.cross_context_in_pure_context",
        "A cross-context call was made in a pure context.",
    ),
    dg(
        "bynk.effect.do_discards_result",
        "A `do` statement was applied to an `Effect[Result[_, _]]`, which would drop an `Err`; bind it, write `let _ <- e` to discard it deliberately, or `do e.detach()` to stop waiting and have a failure reported.",
        &["do_stmt"],
    ),
    dg(
        "bynk.effect.do_in_pure_context",
        "A `do` statement was used in a pure (non-effectful) context.",
        &["do_stmt"],
    ),
    dg(
        "bynk.effect.do_on_non_effect",
        "A `do` statement was applied to a non-`Effect` value.",
        &["do_stmt"],
    ),
    dg(
        "bynk.effect.fn_value_in_pure_context",
        "An effectful function value was called in a pure context; like a capability call, it is legal only where the enclosing body is effectful.",
        &["call"],
    ),
    d(
        "bynk.effect.unbound_effect",
        "An `Effect` value in an effectful body was built but not bound with `<-`, sequenced with `do` or returned — in a plain `let`, a list element or a constructor payload — so it runs eagerly and unawaited (#1658).",
    ),
    d(
        "bynk.email.bad_params",
        "An email `on message` handler does not take exactly one `InboundEmail` parameter.",
    ),
    d(
        "bynk.email.duplicate_address",
        "Two `from email` services claim the same recipient address.",
    ),
    d(
        "bynk.email.duplicate_handler",
        "A `from email` service declares more than one `on message` handler; it has exactly one.",
    ),
    d(
        "bynk.email.invalid_address",
        "A `from email(\"…\")` binding is not a recipient address a service can claim.",
    ),
    d(
        "bynk.email.return_not_email_verdict",
        "An email `on message` handler does not return `Effect[EmailVerdict]`.",
    ),
    d(
        "bynk.embed.binary_content",
        "An `embed` names a file with a NUL byte, so it is binary, not text.",
    ),
    d(
        "bynk.embed.duplicate",
        "An `embed` name is declared more than once in the same unit.",
    ),
    d(
        "bynk.embed.invalid_encoding",
        "An `embed` names a file that is not valid UTF-8.",
    ),
    d(
        "bynk.embed.missing_file",
        "An `embed` names a file that is not in the project's `embeds/` folder.",
    ),
    d(
        "bynk.embed.needs_project",
        "A single-file build met an `embed` declaration, which needs a project's `embeds/` folder.",
    ),
    d(
        "bynk.embed.outside_context_or_commons",
        "An `embed` declaration appears outside a context or commons.",
    ),
    d(
        "bynk.embed.outside_embeds_dir",
        "An `embed` names a file outside the project's `embeds/` folder.",
    ),
    d(
        "bynk.embed.too_large",
        "An `embed` names a file over 256 KiB, or the project's embedded files total over 1 MiB.",
    ),
    d(
        "bynk.embed.unreadable_file",
        "An `embed` names a file in the project's `embeds/` folder that could not be read.",
    ),
    d(
        "bynk.embed.unsafe_path",
        "An `embed` path is empty, absolute, or contains a `..` segment.",
    ),
    d(
        "bynk.event.bad_params",
        "An `on event` handler declared the wrong number of parameters, or a second parameter whose type is not `EventEnvelope` — it takes the event payload and, optionally, the runtime envelope.",
    ),
    d(
        "bynk.event.bad_schema_dispatch",
        "A `via schema(...)` dispatch clause's argument is malformed — it must be a positive `Int` literal, a range of positive `Int` literals, or `_`.",
    ),
    d(
        "bynk.event.bad_schema_version",
        "An event's `@schema(N)` annotation is malformed — `N` must be a single, positive, positional `Int` literal, and `@schema` may appear at most once on an event.",
    ),
    dg(
        "bynk.event.duplicate_handler",
        "A `from Events` service declares more than one `on event` handler; it has exactly one.",
        &["event_handler"],
    ),
    d(
        "bynk.event.emit_not_an_event",
        "`Events.emit[E]` named a type `E` that is declared in this context, but is not itself an `event` — only an `event` type may be emitted.",
    ),
    d(
        "bynk.event.emit_outside_owner",
        "`Events.emit[E]` named an event `E` not declared in the emitting context — only an event's declaring context may emit it.",
    ),
    d(
        "bynk.event.handler_param_type_mismatch",
        "An `on event(e: T)` handler's declared parameter type does not match its `from Events(E)` header's event type.",
    ),
    d(
        "bynk.event.non_additive_schema_change",
        "An event's field shape changed in a way the schema registry cannot evolve additively — a field was removed, retyped, added without a default, or lost a default it previously had. Give the new shape a new event type name, or make the change additive.",
    ),
    d(
        "bynk.event.outside_context",
        "An `event` was declared outside a context.",
    ),
    d(
        "bynk.event.pattern_duplicate_field",
        "A `from Events(E { ... })` subscription pattern listed the same field more than once.",
    ),
    d(
        "bynk.event.pattern_type_mismatch",
        "A `from Events(E { ... })` subscription pattern field's matched value is not compatible with that field's declared type.",
    ),
    d(
        "bynk.event.pattern_unknown_field",
        "A `from Events(E { ... })` subscription pattern named a field that `E` does not declare.",
    ),
    d(
        "bynk.event.pattern_unknown_variant",
        "A `from Events(E { ... })` subscription pattern's variant value names a variant that does not exist on the field's declared sum type.",
    ),
    d(
        "bynk.event.pattern_variant_payload",
        "A `from Events(E { ... })` subscription pattern's variant value names a variant that carries a payload — only nullary variants are admitted, since testing the tag alone would silently ignore the payload.",
    ),
    dg(
        "bynk.event.return_not_effect_unit",
        "An `on event` handler returns an `Effect` of something other than `()`; emission is fire-and-forget, so it returns `Effect[()]`.",
        &["event_handler"],
    ),
    d(
        "bynk.event.schema_version_mismatch",
        "An event's `@schema(N)` annotation disagrees with the version the schema registry computes from the event's build history.",
    ),
    d(
        "bynk.event.unknown_annotation",
        "An `event` declaration carried an `@`-annotation other than `@schema` — event annotations are a closed set.",
    ),
    d(
        "bynk.event.unknown_subscription",
        "A `from Events(E)` subscription named `E`, which is not a declared event in this context or any consumed context.",
    ),
    d(
        "bynk.exec.unavailable",
        "A call to, or reference to, a function that hands work to the execution context (`~>`, `detach`, `within` or `race`, directly or through the functions it calls) appears where no execution context exists: an agent handler, a WebSocket handler or a provider operation.",
    ),
    dg(
        "bynk.expect.faults_not_effect",
        "A fault claim's subject (`expect <call> faults`) is not an effectful call: a value, or a call whose type is not `Effect[_]`.",
        &["faults_expr"],
    ),
    dg(
        "bynk.expect.not_bool",
        "`expect` was given a non-`Bool` predicate.",
        &["expect_expr"],
    ),
    dg(
        "bynk.expect.outside_case",
        "`expect` was used outside a `case` body.",
        &["expect_expr"],
    ),
    dg(
        "bynk.exports.capability_not_provided",
        "An exported capability has no provider in its context.",
        &["exports_decl"],
    ),
    dg(
        "bynk.exports.conflicting_visibility",
        "A type is exported with conflicting visibilities.",
        &["exports_decl"],
    ),
    dg(
        "bynk.exports.derived_capability",
        "`exports capability` names a derived capability, whose instance stays with its context.",
        &["exports_decl"],
    ),
    dg(
        "bynk.exports.duplicate_export",
        "The same name is exported more than once.",
        &["exports_decl"],
    ),
    dg(
        "bynk.exports.duplicate_in_clause",
        "A name appears twice in one `exports` clause.",
        &["exports_decl"],
    ),
    dg(
        "bynk.exports.undeclared_capability",
        "`exports capability` names a capability that is not declared.",
        &["exports_decl"],
    ),
    dg(
        "bynk.exports.undeclared_type",
        "`exports` names a type that is not declared.",
        &["exports_decl"],
    ),
    warn(d(
        "bynk.fault.todo",
        "A `todo(message)` placeholder: the code type-checks but faults when it runs. A non-failing warning, so `check --deny-warnings` refuses unfinished code in CI (warning).",
    )),
    dg(
        "bynk.fixture.binary_content",
        "A suite's `fixture` names a file that contains a NUL byte, so it is binary, not text.",
        &["fixture_decl"],
    ),
    dg(
        "bynk.fixture.duplicate",
        "A suite declares two `fixture`s of the same name.",
        &["fixture_decl"],
    ),
    dg(
        "bynk.fixture.invalid_encoding",
        "A suite's `fixture` names a file that is not valid UTF-8.",
        &["fixture_decl"],
    ),
    dg(
        "bynk.fixture.missing_file",
        "A suite's `fixture` names a file that is not in the project's `fixtures/` folder.",
        &["fixture_decl"],
    ),
    dg(
        "bynk.fixture.outside_fixtures_dir",
        "A suite's `fixture` names a file outside the project's `fixtures/` folder.",
        &["fixture_decl"],
    ),
    dg(
        "bynk.fixture.too_large",
        "A suite's `fixture` names a file over the 4 MiB limit for one fixture file.",
        &["fixture_decl"],
    ),
    dg(
        "bynk.fixture.unreadable_file",
        "A suite's `fixture` names a listed file that could not be read.",
        &["fixture_decl"],
    ),
    dg(
        "bynk.fixture.unsafe_path",
        "A suite's `fixture` path is empty, absolute or contains a `..` segment.",
        &["fixture_decl"],
    ),
    warn(dg(
        "bynk.fixture.unused",
        "A suite declares a `fixture` that none of its cases, properties or helpers uses (warning).",
        &["fixture_decl"],
    )),
    d(
        "bynk.fmt.comment_loss",
        "The formatter would drop a comment, so it leaves the file unchanged.",
    ),
    d(
        "bynk.fmt.mixed_comment_syntax",
        "A file mixes the retired `--` comment or `--|` doc spellings with a line that reads differently under the old and new comment rules (#2203), so the formatter leaves it unchanged.",
    ),
    d(
        "bynk.fmt.roundtrip",
        "The formatter's output does not re-parse to the same program, so it leaves the file unchanged.",
    ),
    dg(
        "bynk.generics.ambiguous_instantiation",
        "A generic function with an `Ord` bound is used as a value, and its type arguments cannot be determined from the type expected at that reference; annotate the expected type (constrained-generics slice 1).",
        &["fn_decl"],
    ),
    dg(
        "bynk.generics.bound_on_capability_op",
        "A generic capability operation's type parameter carries a bound other than `Key` or `Codable`; operations take only those two (constrained-generics slices 2 and 3).",
        &["capability_op"],
    ),
    dg(
        "bynk.generics.codable_op_return",
        "A capability operation\'s `Codable` type parameter appears in its return type. A `Codable` parameter is encoded at the call site; decoding a provider\'s reply raises a question this slice does not answer (what a failed decode of stored data means), so typed reads are deferred (constrained-generics slice 3).",
        &["capability_op"],
    ),
    dg(
        "bynk.generics.duplicate_type_param",
        "A `type` or `fn` declares the same type-parameter name more than once (v0.157, ADR 0183).",
        &[],
    ),
    dg(
        "bynk.generics.generic_non_record",
        "A `type` declaration carries type parameters on a refined, opaque or alias body; only a record (`type Name[T] = { … }`) or sum (`type Name[T] = | … | …`) body may be generic (v0.157/#593, ADRs 0183/0197).",
        &["type_decl"],
    ),
    dg(
        "bynk.generics.generic_sum_embeds",
        "A generic sum type carries an `embeds` clause; embedding into a generic sum is not supported (#593).",
        &["type_decl"],
    ),
    dg(
        "bynk.generics.method_on_generic_type",
        "A *static* method on a generic type never mentions one of the type's parameters in its parameter or return types, so no call could determine it from the arguments or the expected type. Mention the parameter in the signature, or make the method a free function (constrained-generics slice 4; instance methods are supported, #594).",
        &["fn_decl"],
    ),
    dg(
        "bynk.generics.missing_codable_bound",
        "A type parameter is encoded or decoded with `Json.encode`/`Json.decode`, or passed to a `Codable`-bounded parameter of another generic or of a capability operation, but is not declared `Codable` (`[T: Codable]`, or `where A: Codable` for a receiver type\'s parameter). The diagnostic carries a machine-applicable fix that declares `Codable`, beside an existing `Eq`, which `Codable` does not imply (constrained-generics slice 3).",
        &["fn_decl"],
    ),
    dg(
        "bynk.generics.missing_eq_bound",
        "A type parameter is compared with `==`/`!=`, or passed to an `Eq`-bounded parameter of another generic, but is not declared `Eq` (`[T: Eq]`, or `where A: Eq` for a receiver type's parameter). The diagnostic carries a machine-applicable fix (ADR 0430, amended by constrained-generics slice 0).",
        &["fn_decl"],
    ),
    dg(
        "bynk.generics.missing_key_bound",
        "A type parameter keys a `Map` or `Set` (a `Map` key, a `Set` element, a `distinct`/`groupBy`/join key), or is passed to a `Key`-bounded parameter of another generic, but is not declared `Key` (`[K: Key]`, or `where A: Key` for a receiver type's parameter; on the type for a generic type's own parameter). The diagnostic carries a machine-applicable fix that declares `Key`, replacing an existing `Eq`, which `Key` implies (constrained-generics slice 2).",
        &["fn_decl"],
    ),
    dg(
        "bynk.generics.missing_ord_bound",
        "A type parameter is ordered (`<`, `<=`, `>`, `>=`, an ordering kernel key), or passed to an `Ord`-bounded parameter of another generic, but is not declared `Ord` (`[T: Ord]`, or `where A: Ord` for a receiver type's parameter). The diagnostic carries a machine-applicable fix that declares `Ord`, replacing an existing `Eq`, which `Ord` implies (constrained-generics slice 1).",
        &["fn_decl"],
    ),
    dg(
        "bynk.generics.polymorphic_recursion",
        "A generic type refers to itself with arguments other than its own type parameters, in order (`Weird[List[T]]` inside `Weird[T]`, or `Pair[B, A]` inside `Pair[A, B]`), or to a mutually recursive partner with an argument that wraps one of its own parameters (`Inner[List[T]]` inside `Outer[T]`). Such polymorphic recursion generates an unbounded set of instantiations and is refused at the declaration; a self-reference that passes the parameters through unchanged, and a partner applied to the declaration's own parameters or to types that mention none (of any arity), are accepted (#3192).",
        &[],
    ),
    dg(
        "bynk.generics.recursive_generic_at_boundary",
        "A recursive generic type (a record or sum that transitively contains itself, through any wrapper or generic argument) is used where a codec is required: a service or agent handler signature, an agent store field or key, an event field, an actor identity type, a `Json.encode`/`decode` target or a `Codable` witness. It declares and works inside a context, but has no codec yet, so it cannot cross a boundary; when it is reached through another type, the note names the field path (#3192, ADR 0197).",
        &[],
    ),
    warn(dg(
        "bynk.generics.redundant_eq_bound",
        "A type parameter is declared both `Eq` and `Ord` (or `Eq` and `Key`); `Ord` and `Key` imply `Eq`, so the `Eq` is redundant. The diagnostic carries a machine-applicable fix that removes it (constrained-generics slices 1 and 2, warning).",
        &["fn_decl"],
    )),
    dg(
        "bynk.generics.type_arg_count",
        "A user-declared generic type is applied to the wrong number of type arguments, or a generic type is named without its `[…]` arguments (v0.157, ADR 0183).",
        &["applied_type_ref"],
    ),
    dg(
        "bynk.generics.type_arg_mismatch",
        "Inferred or explicit type arguments conflict, have the wrong arity, target a non-generic function, or a type parameter shadows a declared type.",
        &["call"],
    ),
    dg(
        "bynk.generics.uninferable_type_arg",
        "A generic function's type parameter could not be inferred from the arguments and was not given explicitly (`name[T](…)`); a bare generic function also cannot be passed as a value.",
        &["call"],
    ),
    dg(
        "bynk.generics.unknown_constraint",
        "A bound, or a `derives` entry, names something that is not a constraint; the constraints are `Eq`, `Ord`, `Key` and `Codable`.",
        &["type_param"],
    ),
    dg(
        "bynk.generics.unknown_where_param",
        "A method's `where` clause bounds a name that is not a type parameter of the method or of its receiver type.",
        &["where_clause"],
    ),
    dg(
        "bynk.given.cross_context_unknown_capability",
        "`given B.Cap` or `consumes B { Cap }` names a capability the consumed context does not export.",
        &["given_clause"],
    ),
    dg(
        "bynk.given.fn_outside_context",
        "A function in a commons (or a suite) declares `given`; only a context function can.",
        &["given_clause"],
    ),
    dg(
        "bynk.given.fn_unsupported",
        "A `given` clause on a method, on a function that does not return `Effect[T]`, or naming `Events`.",
        &["given_clause"],
    ),
    dg(
        "bynk.given.missing_for_call",
        "A call to a function that declares `given` from a body that does not declare those capabilities.",
        &["given_clause"],
    ),
    dg(
        "bynk.given.undeclared_capability",
        "A handler uses a capability it did not declare with `given`.",
        &["given_clause"],
    ),
    dg(
        "bynk.given.unknown_capability",
        "`given` names a capability that does not exist.",
        &["given_clause"],
    ),
    warn(dg(
        "bynk.given.unused_capability",
        "A `given` capability is never used (warning).",
        &["given_clause"],
    )),
    d(
        "bynk.held.branch_divergence",
        "Branches of a conditional leave a held value (e.g. `Connection[F]`) in inconsistent ownership states — one consumes or stores it, another leaves it owned (§2.9.5, real-time track slice 2).",
    ),
    d(
        "bynk.held.consume_on_borrow",
        "A consuming operation (`close`/`put`/`take`) is called on a *borrowed* held reference — borrows admit only non-consuming operations like `send` (§2.9.3, real-time track slice 2).",
    ),
    d(
        "bynk.held.leak",
        "A held value (`Connection[F]`) is still owned at scope exit — it must be disposed (stored, closed, or transferred) before the handler or function returns (§2.9.1, real-time track slice 2).",
    ),
    d(
        "bynk.held.query_accessor_on_held_map",
        "A key-aware query accessor (`.entries`/`.keys`/`.values`) is used on a held `Map[K, Connection]` — a held resource is iterated with the broadcast ops (`forEach`/`parTraverse`), not a key query.",
    ),
    d(
        "bynk.held.unsupported_map_op",
        "A held `Map[K, Connection]` is given an `update`/`upsert` — a held resource cannot be transformed by a `(Connection) -> Connection` function; use `put`/`get`/`remove` (real-time track slice 3b-ii).",
    ),
    d(
        "bynk.held.unsupported_storage",
        "A held value (`Connection[F]`) is stored in a `Set`/`Log`/`Cache` — held values may only live in `Cell[Option[Connection]]` or `Map[K, Connection]` (§2.9.3, real-time track slice 2).",
    ),
    d(
        "bynk.held.use_after_consume",
        "A held value (`Connection[F]`) is used after a consuming operation (`close`/`put`/`take`) ended its lifetime (§2.9.2, real-time track slice 2).",
    ),
    d(
        "bynk.history.not_an_agent",
        "A `for all run: History[T]` names a `T` that is not an agent — only an agent has handlers to sequence and reachable states to observe (testing track slice 7, ADR 0155).",
    ),
    d(
        "bynk.history.not_generable",
        "A `for all run: History[Agent]` targets an agent with a handler parameter whose type cannot be generated (e.g. a `Matches` refinement), so its call-history cannot be driven (testing track slice 7, ADR 0155).",
    ),
    d(
        "bynk.history.outside_property",
        "`History[Agent]` appears outside a `property`'s `for all` binding — it is a test-only generator, not a value type (testing track slice 7, ADR 0155).",
    ),
    d(
        "bynk.history.restates_invariant",
        "A history property merely re-checks a guarantee a declared `invariant`/`transition` already enforces on every reached state (testing track slice 7, ADR 0155).",
    ),
    d(
        "bynk.http.body_kind_conflict",
        "A handler carries both `@raw` and `@form`, which each fix the wire format of its one `body`.",
    ),
    dg(
        "bynk.http.body_on_get_or_delete",
        "A GET or DELETE handler declares a `body` parameter.",
        &["http_handler"],
    ),
    d(
        "bynk.http.cache_bad_immutable",
        "A `@cache` annotation's `immutable` is not a `Bool` literal.",
    ),
    d(
        "bynk.http.cache_bad_max_age",
        "A `@cache` annotation's `maxAge` is missing or not a positive `Duration` literal.",
    ),
    d(
        "bynk.http.cache_bad_scope",
        "A `@cache` annotation's `scope` is not `public` or `private`.",
    ),
    d(
        "bynk.http.cache_duplicate",
        "A handler carries more than one `@cache` annotation.",
    ),
    d(
        "bynk.http.cache_max_age_fractional_seconds",
        "A `@cache` annotation's `maxAge` is positive but does not resolve to a whole number of seconds.",
    ),
    d(
        "bynk.http.cache_on_non_get",
        "A `@cache` annotation is placed on a handler that is not `on http GET`.",
    ),
    dg(
        "bynk.http.cache_public_reads_cookies",
        "A `@cache(scope: public)` handler takes a `cookies` parameter, so a shared cache could replay one user's response to another.",
        &["http_handler"],
    ),
    d(
        "bynk.http.cache_unknown_arg",
        "A `@cache` annotation has an argument outside the closed set (`maxAge`/`scope`/`immutable`).",
    ),
    warn(dg(
        "bynk.http.cookie_cross_site_without_credentials",
        "A service attaches a `SameSite.Unrestricted` cookie but declares no `cors { credentials: true }`, so a cross-origin `fetch` can never send it back (warning).",
        &["http_handler"],
    )),
    warn(dg(
        "bynk.http.cookie_domain_widens",
        "A cookie literal outside the `__Host-` prefix sets a `domain`, so every subdomain receives it (warning).",
        &["http_handler"],
    )),
    dg(
        "bynk.http.cookie_field_arity",
        "A typed cookie write or deletion (`Record.field.cookie(v)`, `Record.field.expire()`) is given the wrong number of arguments.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.cookie_field_list",
        "A typed cookie write names a `List` field of the cookie record.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.cookie_field_not_cookie_record",
        "A typed cookie write names a record that is not the `cookies` record of an `on http` handler in this unit.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.cookie_field_unknown",
        "A typed cookie write names a field the cookie record does not have.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.cookie_host_prefix",
        "A `__Host-` cookie literal is not `secure`, sets a `domain`, or uses a `path` other than `/`.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.cookie_partitioned_insecure",
        "A `partitioned` cookie literal is not `secure`.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.cookie_samesite_none_insecure",
        "A cookie literal with `sameSite: SameSite.Unrestricted` is not `secure`.",
        &["http_handler"],
    ),
    warn(dg(
        "bynk.http.cookie_script_readable",
        "A cookie literal sets `httpOnly: false`, so page scripts can read it (warning).",
        &["http_handler"],
    )),
    dg(
        "bynk.http.cookie_secure_prefix",
        "A `__Secure-` cookie literal is not `secure`.",
        &["http_handler"],
    ),
    warn(dg(
        "bynk.http.cookie_untyped_has_field",
        "An untyped `Cookie` literal is named like a field of a cookie record of the unit (a warning).",
        &["http_handler"],
    )),
    dg(
        "bynk.http.cookie_value_undecodable",
        "An untyped `Cookie` literal's value is one the record field that reads that cookie would reject.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.cookies_field_not_stringy",
        "A field of an HTTP handler's `cookies` record has a type a cookie cannot carry.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.cookies_not_http",
        "A `RequestCookies` appears on a handler that is not an HTTP route.",
        &["http_handler"],
    ),
    d(
        "bynk.http.cookies_wrong_type",
        "An HTTP handler's `cookies` parameter is not a record type.",
    ),
    d(
        "bynk.http.cors_invalid_field",
        "A `cors` policy field (`headers`/`credentials`/`maxAge`) has the wrong value shape.",
    ),
    d(
        "bynk.http.cors_invalid_origins",
        "A `cors` policy's `origins` is missing, empty, or not a list of string literals.",
    ),
    d(
        "bynk.http.cors_not_http",
        "A `cors { }` policy appears on a service that is not `from http`.",
    ),
    d(
        "bynk.http.cors_unknown_field",
        "A `cors { }` policy declares a field outside the closed set.",
    ),
    d(
        "bynk.http.cors_wildcard_credentials",
        "A `cors` policy combines `credentials: true` with the wildcard origin `[\"*\"]`.",
    ),
    dg(
        "bynk.http.duplicate_rejected",
        "A `from http` service declares `on rejected` more than once.",
        &["rejected_handler"],
    ),
    dg(
        "bynk.http.duplicate_route",
        "Two handlers share the same method and route.",
        &["http_handler"],
    ),
    d(
        "bynk.http.error_status_position",
        "An `ErrorStatus` name is used anywhere but as `RawError`'s first argument.",
    ),
    dg(
        "bynk.http.extra_param",
        "A handler parameter is not a path parameter, `body`, `headers` or `query`.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.form_body_not_record",
        "The `body` of a `@form` handler is not a record type.",
        &["http_handler"],
    ),
    warn(dg(
        "bynk.http.form_bool_without_default",
        "A `Bool` field of a `@form` body has no default, so an unchecked checkbox (which a browser sends as nothing) is a `400` (warning).",
        &["http_handler"],
    )),
    d(
        "bynk.http.form_duplicate",
        "A handler carries more than one `@form` annotation.",
    ),
    dg(
        "bynk.http.form_field_not_stringy",
        "A field of a `@form` body record has a type a form cannot carry.",
        &["http_handler"],
    ),
    d(
        "bynk.http.form_on_bodyless",
        "A `@form` or `@multipart` annotation is placed on a handler that takes no body.",
    ),
    d(
        "bynk.http.form_on_sum_route",
        "A `@form` handler's `by` clause is a sum of actors, whose wrapper reads the body as JSON itself.",
    ),
    d(
        "bynk.http.form_unknown_arg",
        "A `@form` annotation is given an argument; it takes none.",
    ),
    warn(d(
        "bynk.http.header_question_not_found",
        "`?` on `RequestHeaders.get(…)` in an `HttpResult` handler answers `404 NotFound` for a missing header, where `400` is meant (warning; auto-fixable to a `let … else` that answers `BadRequest`).",
    )),
    dg(
        "bynk.http.headers_wrong_type",
        "An HTTP handler's `headers` parameter is not typed `RequestHeaders`.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.invalid_path",
        "An HTTP route path is malformed.",
        &["http_handler"],
    ),
    d(
        "bynk.http.json_duplicate",
        "A handler carries more than one `@json` annotation.",
    ),
    d(
        "bynk.http.json_invalid_arg",
        "A `@json` annotation's `options` argument is missing or is not `tagged` or `plain`.",
    ),
    d(
        "bynk.http.json_invalid_field",
        "A `json { }` policy's `options` is not `tagged` or `plain`.",
    ),
    d(
        "bynk.http.json_not_http",
        "A `json { }` policy appears on a service that is not `from http`.",
    ),
    d(
        "bynk.http.json_on_non_http",
        "A `@json` annotation is placed on a handler that is not `on http`.",
    ),
    d(
        "bynk.http.json_unknown_arg",
        "A `@json` annotation has an argument outside the closed set (`options`).",
    ),
    d(
        "bynk.http.json_unknown_field",
        "A `json { }` policy declares a field outside the closed set (`options`).",
    ),
    d(
        "bynk.http.limit_bad_max_body",
        "A `@limit` annotation's `maxBody` is missing or not a positive `Int` literal.",
    ),
    d(
        "bynk.http.limit_bad_max_file",
        "A `@limit` `maxFile` or `maxParts` is not a positive `Int` literal.",
    ),
    d(
        "bynk.http.limit_duplicate",
        "A handler carries more than one `@limit` annotation.",
    ),
    d(
        "bynk.http.limit_on_bodyless",
        "A `@limit` annotation is placed on a handler that takes no body (a GET or DELETE).",
    ),
    d(
        "bynk.http.limit_on_non_multipart",
        "A `@limit` `maxFile` or `maxParts` is placed on a route that is not `@multipart`.",
    ),
    d(
        "bynk.http.limit_unknown_arg",
        "A `@limit` annotation has an argument outside the closed set (`maxBody`).",
    ),
    d(
        "bynk.http.limits_invalid_field",
        "A `limits` policy field (`maxBody`) has the wrong value shape.",
    ),
    d(
        "bynk.http.limits_not_http",
        "A `limits { }` policy appears on a service that is not `from http`.",
    ),
    d(
        "bynk.http.limits_unknown_field",
        "A `limits { }` policy declares a field outside the closed set.",
    ),
    d(
        "bynk.http.multipart_body_unbounded",
        "A `@multipart` route has no request-body cap (`@limit` or `limits { maxBody }`).",
    ),
    warn(d(
        "bynk.http.multipart_large_cap",
        "A `@multipart` route's body cap is over 32 MiB; the body is buffered with a parsed copy beside it, in a Worker's 128 MB (warning).",
    )),
    dg(
        "bynk.http.path_param_not_stringy",
        "A path parameter's type is not constructible from a string.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.query_field_not_stringy",
        "A field of an HTTP handler's `query` record has a type a query string cannot carry.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.query_not_http",
        "A `RequestQuery` appears on a handler that is not an HTTP route.",
        &["http_handler"],
    ),
    dg(
        "bynk.http.query_wrong_type",
        "An HTTP handler's `query` parameter is neither a record type nor `RequestQuery`.",
        &["http_handler"],
    ),
    d(
        "bynk.http.raw_bad_content_type",
        "A `@raw` annotation's `contentType` is missing or not a `type/subtype` (or `\"*/*\"`) string literal.",
    ),
    d(
        "bynk.http.raw_body_not_bytes",
        "The `body` of a `@raw` handler is not typed `Bytes` or `Stream[Bytes]`.",
    ),
    d(
        "bynk.http.raw_body_unbounded",
        "A `@raw` route has no request-body cap (no `@limit` and no service `limits`).",
    ),
    d(
        "bynk.http.raw_duplicate",
        "A handler carries more than one `@raw` annotation.",
    ),
    d(
        "bynk.http.raw_error_status",
        "`RawError`'s first argument is not an `ErrorStatus` name (a 4xx or 5xx `HttpResult` variant name).",
    ),
    d(
        "bynk.http.raw_on_bodyless",
        "A `@raw` annotation is placed on a handler that takes no body.",
    ),
    d(
        "bynk.http.raw_unknown_arg",
        "A `@raw` annotation has an argument outside the closed set (`contentType`).",
    ),
    dg(
        "bynk.http.rejected_signature",
        "An `on rejected` handler is not `on rejected(why: BoundaryRejection) -> HttpResult[()]` with no `by` and no `given`.",
        &["rejected_handler"],
    ),
    dg(
        "bynk.http.reserved_prefix",
        "A route uses a reserved prefix: `/_bynk/` (internal dispatch) or `/_assets/` (static assets).",
        &["http_handler"],
    ),
    dg(
        "bynk.http.return_not_effect_http_result",
        "An HTTP handler does not return `Effect[HttpResult[T]]`.",
        &["http_handler"],
    ),
    d(
        "bynk.http.security_invalid_field",
        "A `security` policy field (`hsts`/`nosniff`) has the wrong value shape.",
    ),
    d(
        "bynk.http.security_not_http",
        "A `security { }` policy appears on a service that is not `from http`.",
    ),
    d(
        "bynk.http.security_unknown_field",
        "A `security { }` policy declares a field outside the closed set.",
    ),
    d(
        "bynk.http.stream_body_not_raw",
        "A `Stream[Bytes]` `body` is on a handler that is not `@raw`.",
    ),
    d(
        "bynk.http.stream_body_on_sum_route",
        "A route with a streamed `body` has a `by` clause that is a sum of actors, whose wrapper reads the body itself.",
    ),
    d(
        "bynk.http.stream_body_signature",
        "A route with a streamed `body` has a `Signature` actor, which verifies an HMAC over the whole body first.",
    ),
    dg(
        "bynk.http.unbound_path_param",
        "A `:name` route segment has no matching handler parameter.",
        &["http_handler"],
    ),
    d(
        "bynk.http.unknown_handler_annotation",
        "A handler carries an annotation outside the closed set (`@cache`/`@limit`).",
    ),
    d(
        "bynk.index.bad_argument",
        "An `@indexed` argument is not a `by: <field>` label.",
    ),
    warn(d(
        "bynk.index.missing",
        "A query filters a map by equality on a field that is not `@indexed` (a perf-hint warning).",
    )),
    d(
        "bynk.index.unkeyable_key",
        "An `@indexed(by: k)` field is not value-keyable.",
    ),
    d(
        "bynk.index.unknown_key",
        "An `@indexed(by: k)` field is not a field of the map's value type.",
    ),
    warn(d(
        "bynk.index.unused",
        "A declared `@indexed(by: k)` is never used by an equality filter (a hygiene warning).",
    )),
    d(
        "bynk.invariant.cross_agent_reference",
        "An invariant predicate references another agent; invariants are per-agent.",
    ),
    d(
        "bynk.invariant.duplicate_name",
        "An agent declares two invariants with the same name.",
    ),
    d(
        "bynk.invariant.impure_predicate",
        "An invariant predicate uses an effectful or test-only construct.",
    ),
    d(
        "bynk.invariant.not_bool",
        "An invariant predicate does not have type `Bool`.",
    ),
    d(
        "bynk.json.encode_plain_generic",
        "`Json.encodePlain` is given a value whose type mentions a type parameter. A generic value encodes through its `Codable` witness, which always writes `Option` in the tagged form, so plain encoding is available only at a concrete type.",
    ),
    d(
        "bynk.json.nested_option_plain",
        "A plain-JSON route's response type, or the argument of `Json.encodePlain`, contains an `Option[U]` whose payload `U` can itself encode as `null` (`Option[T]` or `()`), so `Some(U)` and `None` would both be written `null`.",
    ),
    dg(
        "bynk.lambda.unannotated_param",
        "A lambda parameter has no type annotation in a position where no function type is expected to infer it from.",
        &["lambda_expr"],
    ),
    dg(
        "bynk.lex.bad_escape",
        "An invalid escape sequence in a string literal.",
        &["string_literal"],
    ),
    dg(
        "bynk.lex.bad_unicode_escape",
        "A `\\u{…}` escape names a surrogate or a value above U+10FFFF.",
        &["string_literal"],
    ),
    dg(
        "bynk.lex.decimal_literal_too_large",
        "A decimal literal has more than 100 fraction digits or more than 1000 characters, the most any `Decimal` may have (#2622).",
        &["decimal_literal"],
    ),
    dg(
        "bynk.lex.float_literal_overflow",
        "A float literal does not fit a finite 64-bit float.",
        &["float_literal"],
    ),
    dg(
        "bynk.lex.integer_overflow",
        "An integer literal is outside the `Int` range, the JS safe integers ±(2^53 − 1).",
        &["number_literal"],
    ),
    dg(
        "bynk.lex.interpolation_too_deep",
        "A string interpolation `\\(…)` nests deeper than the lexer's fixed limit.",
        &["string_literal"],
    ),
    d(
        "bynk.lex.legacy_comment",
        "A `--` line comment, the spelling before #2203; a comment is `//` (auto-fixable; `bynk fmt` rewrites it).",
    ),
    d(
        "bynk.lex.legacy_doc_line",
        "A `--|` one-line doc, the spelling before #2203; a one-line doc is `--- text` (auto-fixable; `bynk fmt` rewrites it).",
    ),
    dg(
        "bynk.lex.multiline_close_not_own_line",
        "A `\"\"\"` inside a multi-line string literal is not on a line of its own.",
        &["string_literal"],
    ),
    dg(
        "bynk.lex.multiline_in_hole",
        "A multi-line string literal appears inside an interpolation hole, which must close on its own line.",
        &["string_literal"],
    ),
    dg(
        "bynk.lex.multiline_indent",
        "A line of a multi-line string literal is indented less than its closing `\"\"\"`.",
        &["string_literal"],
    ),
    dg(
        "bynk.lex.multiline_open",
        "The opening `\"\"\"` of a multi-line string literal is not followed by a newline, or by a language tag (`\"\"\"sql`) and a newline.",
        &["string_literal"],
    ),
    d(
        "bynk.lex.slash_doc",
        "A `///` doc comment first on its line; a Bynk doc is `--- text` or a `---` block (auto-fixable).",
    ),
    d(
        "bynk.lex.unclosed_doc_block",
        "A documentation block is not closed.",
    ),
    d(
        "bynk.lex.unexpected_character",
        "An unexpected character in the source.",
    ),
    dg(
        "bynk.lex.unterminated_interpolation",
        "An interpolation hole `\\(…)` is not closed on its line.",
        &["string_literal"],
    ),
    dg(
        "bynk.lex.unterminated_string",
        "A string literal is not terminated.",
        &["string_literal"],
    ),
    warn(d(
        "bynk.list.deprecated_function",
        "A `bynk.list` free function (`map`/`filter`/`find`/`any`/`all`) is deprecated in favour of the `List` method form (warning; auto-fixable).",
    )),
    d(
        "bynk.locale.multiple_message_bundles",
        "A context consumes `Locale` but its direct `uses` reaches two or more message-bundle commons — there is no single bundle to negotiate against.",
    ),
    d(
        "bynk.messages.format_mismatch",
        "A code's placeholder is formatted as a different ICU kind (plain/plural/select/number/date) across declared locales.",
    ),
    d(
        "bynk.messages.incomplete",
        "A locale is missing a code the reference locale declares.",
    ),
    d(
        "bynk.messages.invalid_locale_tag",
        "A `messages` block's locale tag is not a valid `LocaleTag` (e.g. `messages \"xx\"` where `xx` doesn't match the tag pattern).",
    ),
    d(
        "bynk.messages.malformed_icu_syntax",
        "A message template's ICU placeholder syntax is invalid — unbalanced arm braces, an unknown format keyword, `#` outside a plural arm, a missing mandatory `other` arm, or an explicitly out-of-scope construct (`selectordinal`, `offset:`/`=N`, a CLDR skeleton).",
    ),
    d(
        "bynk.messages.missing_locale_dependency",
        "A commons declaring `messages` doesn't `uses bynk.locale` and/or `uses bynk.locale.types`, which its generated `render` and the types its signature names need.",
    ),
    d(
        "bynk.messages.missing_reference",
        "A message bundle has no `@reference` block.",
    ),
    d(
        "bynk.messages.multiple_reference",
        "A message bundle has more than one `@reference` block.",
    ),
    d(
        "bynk.messages.outside_commons",
        "A `messages` declaration appears outside a commons.",
    ),
    d(
        "bynk.messages.placeholder_mismatch",
        "A locale's template for a code uses a different set of `{name}` placeholders than the reference locale's.",
    ),
    warn(d(
        "bynk.mysql.sql_invalid",
        "A literal `MySql` statement does not parse with the MySQL dialect of a pure-Rust SQL parser, or is more than one command, in a project that has a `schema.sql` (warning: the parser is not MySQL's own, so it may refuse valid SQL).",
    )),
    d(
        "bynk.mysql.sql_param_count",
        "A literal `MySql` statement has a different number of `?` placeholders than the list literal passed with it holds, in a project that has a `schema.sql`.",
    ),
    d(
        "bynk.namespace.reserved",
        "A user unit is named `bynk` or `bynk.*`; the `bynk` root is reserved for the toolchain.",
    ),
    d(
        "bynk.observe.impure_with",
        "A `with` predicate uses an effectful or test-only construct; it must be pure.",
    ),
    d(
        "bynk.observe.not_a_seam",
        "An observation targets a capability the unit under test does not consume.",
    ),
    d(
        "bynk.observe.trace_outside_test",
        "`trace(Cap.op)` appears outside a `case` body.",
    ),
    d(
        "bynk.observe.unknown_op",
        "An observation names an operation the capability does not declare.",
    ),
    d(
        "bynk.observe.with_not_bool",
        "A `with` predicate does not have type `Bool`.",
    ),
    d(
        "bynk.package.alias_shadows_unit",
        "A dependency alias in `[dependencies]` is also the first segment of one of the project's own units, so `<alias>.…` would be ambiguous.",
    ),
    d(
        "bynk.package.checksum_mismatch",
        "A git dependency's store entry, or a freshly fetched commit, doesn't hash to the checksum `bynk.lock` records; `bynk` refuses to build it.",
    ),
    d(
        "bynk.package.dependency_cycle",
        "The package graph has a cycle: a dependency depends, directly or not, on itself or on the project.",
    ),
    d(
        "bynk.package.dependency_not_found",
        "A path dependency's directory has no readable `bynk.toml`, its `bynk.toml` is invalid, or it has no `[package]`.",
    ),
    d(
        "bynk.package.dev_dependency_in_source",
        "A commons, context or adapter reaches a `[dev-dependencies]` alias, which only a suite may use.",
    ),
    d(
        "bynk.package.duplicate_source",
        "One package is reached from two different directories; a build holds one copy of each package.",
    ),
    d(
        "bynk.package.git_failed",
        "`git` couldn't clone, fetch or check out a git dependency, or the revision it names doesn't exist.",
    ),
    d(
        "bynk.package.lock_invalid",
        "`bynk.lock` can't be read: it isn't valid TOML, has no `version`, is a version this toolchain doesn't read, or locks a package to something that isn't a commit.",
    ),
    d(
        "bynk.package.lock_out_of_date",
        "`--locked` was given, and resolving the manifests would change `bynk.lock`.",
    ),
    d(
        "bynk.package.name_mismatch",
        "A path dependency's `package` names a different package than the directory's `[package] name`.",
    ),
    d(
        "bynk.package.not_fetched",
        "A git dependency has no `bynk.lock` entry, or its store entry is missing; `bynk fetch` fetches it (refused under `--offline`).",
    ),
    d(
        "bynk.package.unit_not_exported",
        "A `uses` or `consumes` reaches a unit its dependency declares but doesn't list in `[package] exports`.",
    ),
    d(
        "bynk.package.unknown_unit",
        "A `uses` or `consumes` names, through a dependency alias, a unit the dependency doesn't declare.",
    ),
    d(
        "bynk.package.unsafe_source",
        "A git dependency's URL or revision looks like a command-line option, or uses a transport other than `https`, `ssh` (or `file`, from the project's own manifests).",
    ),
    d(
        "bynk.package.unsupported_in_dependency",
        "A dependency declares an `asset` or an `embed`, which would resolve against the consuming project's `assets/` or `embeds/`; a per-package namespace for either isn't designed yet.",
    ),
    d(
        "bynk.package.version_conflict",
        "One package is reached at two different git revisions (or from git and a path); a build, or a workspace, holds one copy of each package.",
    ),
    dg(
        "bynk.par.invalid_statement",
        "A `par { … }` block holds a statement other than `let x <- e` or `do e`: a refutable `let … else`, a plain `let`, `~>`, `:=` or a store write, `expect`, `advance`, a nested `par`, or anything that is not a statement.",
        &["par_stmt"],
    ),
    dg(
        "bynk.par.question_in_join",
        "A `?` appears inside a `par { … }` binding. `?` takes a `Result` value, not an effect, so a binding's own result is unwrapped after the block (`let user = userR?`), where the `?` lines run in source order; a `?` on an argument is computed before the block.",
        &["par_stmt"],
    ),
    dg(
        "bynk.par.sibling_name",
        "A `par { … }` binding mentions a name another binding in the same block binds, or binds a name twice. The bindings run together, so none can read a sibling's result; the rule holds even where the name would resolve to an outer binding.",
        &["par_stmt"],
    ),
    warn(dg(
        "bynk.par.single_binding",
        "A `par { … }` block has a single binding, which is a plain `let x <- e` with extra syntax (warning).",
        &["par_stmt"],
    )),
    d(
        "bynk.par.store_write_in_closure",
        "A closure passed to `parTraverse`, `parTraverseAll` or `parTraverseTry` writes a `store` field (`put`, `remove`, `update`, `upsert`, `add`, `append` or `:=`). Its branches run concurrently, and a `get` then a `put` across them loses updates, so store writes stay sequential, as they do in a `par { … }` block.",
    ),
    dg(
        "bynk.parse.bare_expression_statement",
        "An expression stands alone as a statement, followed by more of the block; a block has no expression statements, so its value would be discarded. Bind it with `let _ <-` (an effect) or `let _ =`.",
        &["block"],
    ),
    d(
        "bynk.parse.clause_after_statement",
        "A `stub` or `env` clause came after a statement in a test case; they lead the case body.",
    ),
    dg(
        "bynk.parse.const_missing_type",
        "A `const` declaration has no type annotation; a constant's type is always written (`const Name: Type = …`).",
        &["const_decl"],
    ),
    dg(
        "bynk.parse.consumes_after_decls",
        "`consumes` appears after other declarations.",
        &["consumes_decl"],
    ),
    d(
        "bynk.parse.dangling_handler_annotation",
        "A handler-position annotation (e.g. `@cache`) is not followed by an `on` handler.",
    ),
    d(
        "bynk.parse.doc_block_contains_code",
        "A documentation block's content (outside Markdown code fences) parses as declarations, so a pair of dash dividers has turned code into documentation.",
    ),
    dg(
        "bynk.parse.duplicate_cors",
        "A service declares more than one `cors { }` policy.",
        &["service_decl"],
    ),
    dg(
        "bynk.parse.duplicate_json",
        "A service declares more than one `json { }` policy.",
        &["service_decl"],
    ),
    dg(
        "bynk.parse.duplicate_limits",
        "A service declares more than one `limits { }` policy.",
        &["service_decl"],
    ),
    dg(
        "bynk.parse.duplicate_security",
        "A service declares more than one `security { }` policy.",
        &["service_decl"],
    ),
    dg(
        "bynk.parse.empty_agent",
        "An `agent` body is empty.",
        &["agent_decl"],
    ),
    dg(
        "bynk.parse.empty_capability",
        "A `capability` body is empty.",
        &["capability_decl"],
    ),
    d(
        "bynk.parse.empty_interpolation",
        "An interpolation hole `\\(…)` contains no expression.",
    ),
    dg(
        "bynk.parse.empty_match",
        "A `match` has no arms.",
        &["match_expr"],
    ),
    dg(
        "bynk.parse.empty_service",
        "A `service` body is empty.",
        &["service_decl"],
    ),
    d(
        "bynk.parse.event_pattern_empty",
        "A `from Events(E { ... })` subscription pattern listed no fields — use `from Events(E)` (no braces) for an unfiltered subscription.",
    ),
    dg(
        "bynk.parse.expected_agent_key",
        "Expected a `key` declaration in an agent.",
        &["agent_decl"],
    ),
    d(
        "bynk.parse.expected_agent_storage",
        "An agent declares no storage — it has no `store` fields.",
    ),
    dg(
        "bynk.parse.expected_base_type",
        "Expected a base type.",
        &["base_type"],
    ),
    dg(
        "bynk.parse.expected_capability_op",
        "Expected a capability operation.",
        &["capability_op"],
    ),
    d("bynk.parse.expected_expression", "Expected an expression."),
    dg(
        "bynk.parse.expected_handler",
        "Expected a handler.",
        &["handler"],
    ),
    d("bynk.parse.expected_item", "Expected a declaration."),
    dg(
        "bynk.parse.expected_predicate",
        "Expected a refinement predicate.",
        &["refinement"],
    ),
    dg(
        "bynk.parse.expected_provider_op",
        "Expected a provider operation.",
        &["provider_op"],
    ),
    d("bynk.parse.expected_token", "Expected a specific token."),
    d("bynk.parse.expected_type", "Expected a type."),
    d(
        "bynk.parse.expected_unit_header",
        "Expected a `commons` or `context` header.",
    ),
    dg(
        "bynk.parse.expected_visibility",
        "Expected a visibility keyword.",
        &["exports_decl"],
    ),
    dg(
        "bynk.parse.exports_after_decls",
        "`exports` appears after other declarations.",
        &["exports_decl"],
    ),
    d(
        "bynk.parse.extra_tokens",
        "Unexpected tokens after an otherwise complete construct.",
    ),
    dg(
        "bynk.parse.generic_arg_count",
        "Wrong number of generic type arguments.",
        &["generic_type_ref"],
    ),
    dg(
        "bynk.parse.handler_in_agent",
        "A protocol handler (`on GET`/`schedule`/`message`) was declared in an agent.",
        &["handler"],
    ),
    d(
        "bynk.parse.invariant_after_handler",
        "An `invariant` was declared after a handler; invariants precede handlers.",
    ),
    dg(
        "bynk.parse.keyword_field_shorthand",
        "A reserved keyword was used as a field shorthand, which would bind a variable of that name.",
        &["identifier"],
    ),
    dg(
        "bynk.parse.lambda_pattern_annotation",
        "A lambda parameter with a record pattern carries a type annotation; the pattern's type name already gives the parameter's type.",
        &["lambda_param"],
    ),
    dg(
        "bynk.parse.lambda_pattern_not_record",
        "A lambda parameter is a pattern other than a record pattern; only an irrefutable record pattern may stand in for a name.",
        &["lambda_param"],
    ),
    dg(
        "bynk.parse.lambda_pattern_refutable",
        "A lambda parameter's record pattern can fail to match some value of its type, and a lambda has no `else` to fall back on.",
        &["lambda_param"],
    ),
    dg(
        "bynk.parse.let_else_on_binding",
        "An `else` follows a plain `let name = …` binding, which matches every value, so the `else` could never run.",
        &["let_stmt", "effect_let_stmt"],
    ),
    hint(d(
        "bynk.parse.let_else_staircase",
        "Nested `match`es on `Option`/`Result` that each carry on in one arm and give up in the other, which `let … else` writes flat. A hint with a machine-applicable rewrite; never fails a build and is hidden from the terminal (hint).",
    )),
    dg(
        "bynk.parse.let_pattern_annotation",
        "A `let` with a pattern carries a type annotation; the pattern's names take their types from the value.",
        &["let_else_stmt"],
    ),
    dg(
        "bynk.parse.let_pattern_without_else",
        "A `let` binds a pattern but has no `else` saying what the block produces when the value does not match; only a record pattern that cannot fail may omit it.",
        &["let_else_stmt"],
    ),
    dg(
        "bynk.parse.list_rest_not_last",
        "A `..` in a list pattern is not the final element; suffix forms such as `[.., last]` are not available.",
        &["list_pattern"],
    ),
    dg(
        "bynk.parse.malformed_float_literal",
        "A float literal is missing a digit on one side of the `.` (`1.`, `.5`).",
        &["float_literal"],
    ),
    dg(
        "bynk.parse.match_ends_block",
        "A `match` ends the block it is in, because a block has no expression statements; a second `match` straight after it cannot follow.",
        &["block"],
    ),
    d(
        "bynk.parse.migrate_after_handler",
        "A `migrate` step is declared after an agent handler; migration steps precede the handlers.",
    ),
    d(
        "bynk.parse.migrate_item",
        "A `migrate` step's body holds something other than `field := value`, `let` or `drop field`.",
    ),
    d(
        "bynk.parse.multiple_docs",
        "A declaration or member has two docs directly above it; a target has one doc, so a doc of more than one line is a `---` block.",
    ),
    d(
        "bynk.parse.nesting_too_deep",
        "An expression or type nests deeper than the parser's fixed limit.",
    ),
    d(
        "bynk.parse.no_remainder_operator",
        "`%` is not an operator; use `Int.rem` (the dividend's sign) or `Int.mod` (the divisor's).",
    ),
    dg(
        "bynk.parse.non_associative",
        "A non-associative operator was chained (e.g. `a == b == c`).",
        &["binary_expr"],
    ),
    warn(d(
        "bynk.parse.orphan_doc_block",
        "A documentation block is not attached to a declaration (warning).",
    )),
    dg(
        "bynk.parse.policy_order",
        "A service's `cors`/`security`/`limits` policy is out of order or follows a handler; they open the body in the order `cors`, `security`, `limits`, `json`.",
        &["service_decl"],
    ),
    dg(
        "bynk.parse.record_pattern_in_is_unparenthesised",
        "A record pattern after `is` is not parenthesised — write `x is (T { f, .. })`.",
        &["record_pattern"],
    ),
    dg(
        "bynk.parse.record_pattern_without_rest",
        "A record pattern does not end with `..`; every record pattern is open.",
        &["record_pattern"],
    ),
    dg(
        "bynk.parse.refined_pattern_inner",
        "A refined pattern's inner form is something other than `_`.",
        &["refined_pattern"],
    ),
    dg(
        "bynk.parse.reserved_keyword",
        "A reserved keyword was used as an identifier.",
        &["identifier"],
    ),
    dg(
        "bynk.parse.self_outside_method",
        "`self` is the first parameter of a free function; only a method declaration, `fn Type.method(self, …)`, has a receiver.",
        &["self_expr"],
    ),
    d(
        "bynk.parse.storage_after_phase",
        "Agent storage (`state` / `store`) is declared after the invariants or handlers.",
    ),
    d(
        "bynk.parse.transition_after_handler",
        "A `transition` is declared after an agent handler; step invariants precede the handlers.",
    ),
    d(
        "bynk.parse.unexpected_adapter",
        "An `adapter` appeared where it is not allowed.",
    ),
    dg(
        "bynk.parse.unexpected_context",
        "A `context` appeared where it is not allowed.",
        &["context_decl"],
    ),
    d("bynk.parse.unexpected_eof", "Unexpected end of input."),
    dg(
        "bynk.parse.unexpected_suite",
        "A `suite` appeared where it is not allowed.",
        &["suite_decl"],
    ),
    d(
        "bynk.parse.unknown_effect_method",
        "An unknown method on `Effect`.",
    ),
    dg(
        "bynk.parse.unknown_handler_kind",
        "An unknown handler form (expected `call`, an HTTP method, `schedule`, or `message`).",
        &["handler"],
    ),
    dg(
        "bynk.parse.unknown_predicate",
        "An unknown refinement predicate.",
        &["predicate_name"],
    ),
    d(
        "bynk.parse.unknown_tier",
        "A `case`/`suite` `as <tier>` clause names something other than `unit`, `integration`, or `system`.",
    ),
    dg(
        "bynk.parse.uses_after_decls",
        "`uses` appears after other declarations.",
        &["uses_decl"],
    ),
    dg(
        "bynk.parse.variant_name_case",
        "A sum-type or enum variant name is not capitalised.",
        &["sum_variant", "enum_type"],
    ),
    warn(d(
        "bynk.postgres.sql_invalid",
        "A literal `Postgres` statement does not parse with the Postgres dialect of a pure-Rust SQL parser, or is more than one command, in a project that has a `schema.sql` (warning: the parser is not Postgres' own, so it may refuse valid SQL).",
    )),
    d(
        "bynk.postgres.sql_param_count",
        "A literal `Postgres` statement uses a different number of parameters (its highest `$n`) than the list literal passed with it holds, in a project that has a `schema.sql`.",
    ),
    d(
        "bynk.project.contracts_lock_corrupt",
        "`bynk.contracts.lock` (the older service contracts a callee still accepts) is missing its version field, empty, truncated, unparseable, or of an unsupported version — restore it from version control rather than deleting it, since a callee that forgets the contracts it accepts refuses its live callers.",
    ),
    warn(d(
        "bynk.project.contracts_lock_oversized",
        "`bynk.contracts.lock` has one service accepting more older contracts than the build keeps comfortably (warning).",
    )),
    warn(d(
        "bynk.project.contracts_lock_stale",
        "`bynk.contracts.lock` lists an older contract the service no longer accepts, because it is unreadable or no longer compatible with the contract the service has now; the entry is ignored (warning).",
    )),
    warn(d(
        "bynk.project.duplicate_compatibility_flag",
        "`bynk.toml`'s `[workers] compatibility_flags` lists a flag twice, or one every Bynk Worker already has; the duplicate is dropped (warning).",
    )),
    d(
        "bynk.project.duplicate_module_doc",
        "More than one file of a multi-file unit carries a module doc.",
    ),
    d(
        "bynk.project.export_context",
        "`bynk.toml`'s `[package] exports` lists a context: a package shares commons and adapters, never a context, which belongs to the project that deploys it.",
    ),
    d(
        "bynk.project.export_unknown",
        "`bynk.toml`'s `[package] exports` lists a name that isn't a commons or adapter the project declares.",
    ),
    d(
        "bynk.project.file_and_directory",
        "A unit exists as both a file and a directory.",
    ),
    d(
        "bynk.project.inconsistent_commons_name",
        "A source file's path does not match its declared name.",
    ),
    d(
        "bynk.project.kind_conflict",
        "A name is declared as both a commons and a context.",
    ),
    d(
        "bynk.project.no_root",
        "No project root could be determined.",
    ),
    d(
        "bynk.project.no_sources",
        "The project contains no source files.",
    ),
    d(
        "bynk.project.read_failed",
        "A source file could not be read.",
    ),
    d(
        "bynk.project.schema_registry_corrupt",
        "`bynk.schema.lock` (the schema registry for events and agent state) is missing its version field, empty, truncated, or otherwise unparseable — restore it from version control rather than deleting it, since deleting it would silently re-baseline every event's and agent's history.",
    ),
    warn(d(
        "bynk.project.unstable_not_experimental",
        "`bynk.toml`'s `[unstable] allow` lists an item that nothing experimental lies in or under: it is stable now, so the entry can go (warning).",
    )),
    d(
        "bynk.project.unstable_unknown_item",
        "`bynk.toml`'s `[unstable] allow` lists a path that names no standard-library unit, exported item or operation.",
    ),
    dg(
        "bynk.property.restates_refinement",
        "A `property` merely re-checks a refinement its type already guarantees.",
        &["for_all"],
    ),
    dg(
        "bynk.property.where_not_bool",
        "A `for all ... where` filter does not type to `Bool`.",
        &["for_all"],
    ),
    dg(
        "bynk.provider.agent_call_undeclared_capability",
        "A provider calls an agent handler that needs a capability the provider's `given` does not list.",
        &["provider_decl"],
    ),
    dg(
        "bynk.provider.dependency_cycle",
        "Providers form a capability dependency cycle through `given`.",
        &["provider_decl"],
    ),
    dg(
        "bynk.provider.extra_operation",
        "A `provides` block implements an operation not in the capability.",
        &["provider_decl"],
    ),
    dg(
        "bynk.provider.generic_op_requires_external",
        "A Bynk-bodied `provides` implements a capability operation that declares its own type parameter — only an external (bodiless) provider can.",
        &["provider_decl"],
    ),
    dg(
        "bynk.provider.missing_operation",
        "A `provides` block is missing a capability operation.",
        &["provider_decl"],
    ),
    dg(
        "bynk.provider.outside_context",
        "`provides` was declared outside a context.",
        &["provider_decl"],
    ),
    dg(
        "bynk.provider.signature_mismatch",
        "A `provides` operation's signature does not match the capability.",
        &["provider_decl"],
    ),
    dg(
        "bynk.provider.unknown_capability",
        "`provides` names a capability that does not exist.",
        &["provider_decl"],
    ),
    d(
        "bynk.query.join_key_mismatch",
        "A `joinOn`/`leftJoin` left and right key function return different types.",
    ),
    dg(
        "bynk.query.sum_needs_numeric",
        "A `sum`/`average` key function does not return a numeric type (`Int`, `Float`, or `Duration`; `sum` also takes a `Decimal`, `average` does not).",
        &[],
    ),
    dg(
        "bynk.queue.bad_params",
        "An `on message` handler does not take exactly one `message` parameter.",
        &["queue_handler"],
    ),
    dg(
        "bynk.queue.duplicate_consumer",
        "Two `on message` handlers consume the same queue.",
        &["queue_handler"],
    ),
    dg(
        "bynk.queue.invalid_name",
        "A `from queue(\"…\")` binding has an empty queue name, or one Cloudflare refuses.",
        &["queue_handler"],
    ),
    dg(
        "bynk.queue.return_not_queue_result",
        "An `on message` handler does not return `Effect[QueueResult]`.",
        &["handler"],
    ),
    dg(
        "bynk.race.in_agent_handler",
        "`a.race(b)` was used in an agent handler, which has no execution context to hand the losing effect to.",
        &["method_call"],
    ),
    dg(
        "bynk.race.in_pure_context",
        "`a.race(b)` was used in a pure (non-effectful) context.",
        &["method_call"],
    ),
    dg(
        "bynk.race.outside_handler",
        "`a.race(b)` was used outside a service handler's body and outside a function (in a provider operation, a WebSocket handler or a test `case` body), where no execution context exists.",
        &["method_call"],
    ),
    dg(
        "bynk.record_spread.field_type_mismatch",
        "A record-spread override has the wrong type for the field.",
        &["record_spread"],
    ),
    dg(
        "bynk.record_spread.non_record_base",
        "The base of a record spread is not a record.",
        &["record_spread"],
    ),
    dg(
        "bynk.record_spread.type_mismatch",
        "A record spread's base is a different record type.",
        &["record_spread"],
    ),
    dg(
        "bynk.record_spread.unknown_field",
        "A record spread overrides a field the record does not have.",
        &["record_spread"],
    ),
    dg(
        "bynk.refine.literal_violates",
        "A literal does not satisfy the refined type's predicate.",
        &["refined_type"],
    ),
    d(
        "bynk.registry.account_refused",
        "`bynk login`, `bynk logout` or a `bynk org` command didn't go through: you aren't signed in or your session expired, the registry has no account API (or is read through a mirror), the sign-in was declined or its code expired, or the registry refused the request (a taken or reserved name, not the owner, an unknown member or token).",
    ),
    d(
        "bynk.registry.bad_archive",
        "A registry archive is malformed or hostile: not a `bynk-tree-v1` stream, a path that is absolute or climbs out of the package, an out-of-order or repeated file, or past the size limits.",
    ),
    d(
        "bynk.registry.bad_signature",
        "An index line isn't signed by a key the trusted key's rotation chain reaches, or its signed registry `id` or package `name` isn't the one asked for (a forged or replayed line).",
    ),
    d(
        "bynk.registry.http_failed",
        "A registry couldn't be read: unreachable, an error status, a refused or missing token, a redirect from https to http, too many redirects, or a response past the size limit.",
    ),
    d(
        "bynk.registry.key_mismatch",
        "A registry's keys don't lead from the key trusted for it (pinned in `[registries]`, or recorded in `bynk.lock`), or it now declares a different `id`.",
    ),
    d(
        "bynk.registry.nonconformant",
        "`bynk registry conformance` found a registry breaking the protocol: a check failed (a wrong status, an unsigned or mis-signed line, a checksum that isn't the tree sent, a refusal without its JSON body, …); each failure names the check and the protocol section it holds the registry to.",
    ),
    d(
        "bynk.registry.publish_refused",
        "`bynk publish` or `bynk yank` didn't go through: the project isn't a versioned package, it has a path or git dependency, its registry has no publish API (or is read through a mirror), or the registry refused the request (no or wrong token, an existing version, an invalid upload).",
    ),
    d(
        "bynk.registry.source_mismatch",
        "`bynk.lock` locks a package to one registry, but its organisation now routes to another; a misrouted organisation fails rather than resolving a same-named package elsewhere.",
    ),
    d(
        "bynk.registry.unavailable",
        "No registry serves a package: its organisation isn't listed by any `[registries]` entry (the first-party registry isn't live yet), the registry has no `config.json`, or it has no such package or archive.",
    ),
    d(
        "bynk.registry.withdrawn",
        "`bynk.lock` locks a registry version that its registry withdrew (a takedown: yanked, and its archive deleted), and its tree isn't in the store.",
    ),
    dg(
        "bynk.requires.conflicting_range",
        "Two adapter `binding … requires { … }` entries give different version ranges for one npm package.",
        &["binding_decl"],
    ),
    dg(
        "bynk.requires.unapproved",
        "A reached adapter of a git or registry dependency requires an npm package, at a range, that the project hasn't approved in `bynk.lock`; review it and run `bynk approve <package>`.",
        &["binding_decl"],
    ),
    dg(
        "bynk.requires.unpinned_dependency",
        "An adapter `binding … requires { … }` entry has an unpinned version range.",
        &["binding_decl"],
    ),
    d(
        "bynk.resolve.ambiguous_variant",
        "A bare variant name is declared by several sum types in scope, and no expected type picks one.",
    ),
    dg(
        "bynk.resolve.arity_mismatch",
        "A function was called with the wrong number of arguments.",
        &["call"],
    ),
    dg(
        "bynk.resolve.bad_const_initialiser",
        "A `const` initialiser is not a static value of the declared type: only literals, constructors over static values, `T.unsafe(lit)`, `List.empty()`/`Map.empty()`/`Set.empty()`, `Duration` literals and references to other constants are allowed.",
        &["const_decl"],
    ),
    dg(
        "bynk.resolve.const_cycle",
        "Constants refer to each other in a cycle, so none of them has a value.",
        &["const_decl"],
    ),
    dg(
        "bynk.resolve.const_name_case",
        "A constant's name is not upper-camel case (`MaxRetries`, not `maxRetries`).",
        &["const_decl"],
    ),
    dg(
        "bynk.resolve.const_outside_context_or_commons",
        "A `const` declaration appears outside a context or commons.",
        &["const_decl"],
    ),
    d("bynk.resolve.duplicate_actor", "Two actors share a name."),
    dg(
        "bynk.resolve.duplicate_agent",
        "Two agents share a name.",
        &["agent_decl"],
    ),
    dg(
        "bynk.resolve.duplicate_capability",
        "Two capabilities share a name.",
        &["capability_decl"],
    ),
    dg(
        "bynk.resolve.duplicate_const",
        "Two constants share a name.",
        &["const_decl"],
    ),
    dg(
        "bynk.resolve.duplicate_field",
        "A record, or one variant's payload, declares a field twice.",
        &["record_type"],
    ),
    dg(
        "bynk.resolve.duplicate_field_init",
        "A record construction initialises a field twice.",
        &["record_construction"],
    ),
    dg(
        "bynk.resolve.duplicate_fn",
        "Two functions share a name.",
        &["fn_decl"],
    ),
    d(
        "bynk.resolve.duplicate_message_code",
        "A message bundle declares the same code twice in one block.",
    ),
    d(
        "bynk.resolve.duplicate_message_locale",
        "Two `messages` blocks in one bundle declare the same locale tag.",
    ),
    dg(
        "bynk.resolve.duplicate_method",
        "Two methods share a name.",
        &["fn_decl"],
    ),
    dg(
        "bynk.resolve.duplicate_param",
        "A parameter name is repeated.",
        &["param"],
    ),
    dg(
        "bynk.resolve.duplicate_provider",
        "A capability is provided more than once.",
        &["provider_decl"],
    ),
    dg(
        "bynk.resolve.duplicate_service",
        "Two services share a name.",
        &["service_decl"],
    ),
    dg(
        "bynk.resolve.duplicate_type",
        "Two types share a name.",
        &["type_decl"],
    ),
    dg(
        "bynk.resolve.duplicate_variant",
        "A sum type declares a variant twice.",
        &["sum_type"],
    ),
    d(
        "bynk.resolve.fn_without_call",
        "A function was referenced without being called.",
    ),
    dg(
        "bynk.resolve.let_shadows_fn",
        "A `let` binding shadows a function.",
        &["let_stmt"],
    ),
    dg(
        "bynk.resolve.let_shadows_type",
        "A `let` binding shadows a type.",
        &["let_stmt"],
    ),
    d(
        "bynk.resolve.method_unknown_type",
        "A method is defined on an unknown type.",
    ),
    dg(
        "bynk.resolve.missing_field",
        "A record construction omits a required field.",
        &["record_construction"],
    ),
    d(
        "bynk.resolve.name_conflict",
        "Two declarations share a name.",
    ),
    dg(
        "bynk.resolve.not_a_record_type",
        "Record syntax was used on a non-record type.",
        &["record_construction"],
    ),
    dg(
        "bynk.resolve.opaque_record_construction",
        "An opaque type was constructed with record syntax.",
        &["record_construction"],
    ),
    dg(
        "bynk.resolve.param_as_function",
        "A value (such as a parameter) was called as a function.",
        &["call"],
    ),
    dg(
        "bynk.resolve.recursive_record_field",
        "A record directly contains a field of its own type.",
        &["record_type"],
    ),
    dg(
        "bynk.resolve.reserved_builtin_type",
        "A type declaration reuses a compiler-known built-in type name.",
        &["type_decl"],
    ),
    dg(
        "bynk.resolve.reserved_host_name",
        "A declaration is named `globalThis`, which the generated TypeScript uses to reach host globals.",
        &["type_decl", "fn_decl", "agent_decl", "provider_decl"],
    ),
    dg(
        "bynk.resolve.reserved_payload_field",
        "A sum variant declares a payload field named `kind`, the variant's wire discriminant.",
        &["sum_type"],
    ),
    dg(
        "bynk.resolve.self_outside_method",
        "`self` referenced outside a method or handler.",
        &["self_expr"],
    ),
    dg(
        "bynk.resolve.type_as_function",
        "A type name was called as if it were a function.",
        &["call"],
    ),
    d(
        "bynk.resolve.type_in_expr",
        "A type name was used where a value is expected.",
    ),
    dg(
        "bynk.resolve.unconsumed_context",
        "A context's service was called, or its capability named in `given`, without a `consumes` declaration.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.resolve.unknown_field",
        "A record construction names a field the record type does not declare.",
        &["record_construction"],
    ),
    dg(
        "bynk.resolve.unknown_function",
        "Called a function that does not exist.",
        &["call"],
    ),
    d(
        "bynk.resolve.unknown_name",
        "Referenced a name that is not in scope.",
    ),
    dg(
        "bynk.resolve.unknown_static_member",
        "Referenced an unknown static member (e.g. `T.x`).",
        &["field_access"],
    ),
    d(
        "bynk.resolve.unknown_type",
        "Referenced a type that does not exist.",
    ),
    warn(d(
        "bynk.secrets.computed_name",
        "A `bynk.Secrets` read names its secret with a computed expression rather than a literal, so `bynk deploy` cannot plan it (warning).",
    )),
    warn(dg(
        "bynk.send.deprecated",
        "The `~>` send is deprecated in favour of `do e.detach()` (warning; auto-fixable).",
        &["effect_send_stmt"],
    )),
    dg(
        "bynk.send.in_agent_handler",
        "A `~>` send was used in an agent handler, which has no execution context to keep a fire-and-forget send alive.",
        &["effect_send_stmt"],
    ),
    dg(
        "bynk.send.in_pure_context",
        "A `~>` send was used in a pure (non-effectful) context.",
        &["effect_send_stmt"],
    ),
    dg(
        "bynk.send.non_effect",
        "A `~>` send was applied to a non-`Effect` value.",
        &["effect_send_stmt"],
    ),
    dg(
        "bynk.send.requires_unit",
        "A `~>` send targets an operation whose reply is not `Effect[()]`.",
        &["effect_send_stmt"],
    ),
    dg(
        "bynk.send.store_op",
        "A `~>` send targets an agent store operation, which is part of the handler's atomic commit and cannot be sent without waiting.",
        &["effect_send_stmt"],
    ),
    dg(
        "bynk.service.missing_from",
        "A `from`-less service has a handler other than `on call`.",
        &["service_decl"],
    ),
    dg(
        "bynk.service.mixed_protocols",
        "A service mixes handler forms that do not match its `from <protocol>`.",
        &["service_decl"],
    ),
    dg(
        "bynk.service.outside_context",
        "A `service` was declared outside a context.",
        &["service_decl"],
    ),
    dg(
        "bynk.service.return_not_effect",
        "A service handler's return type is not an `Effect`.",
        &["service_decl"],
    ),
    dg(
        "bynk.service.unknown_protocol",
        "A `from <protocol>` names an unknown protocol (e.g. a transport like Kafka).",
        &["service_decl"],
    ),
    d(
        "bynk.service.unknown_via_clause",
        "A `via <name>(...)` clause on a `from Events(...)` header named something other than `schema` — `via` clauses are a closed set, and only `via schema(...)` exists today.",
    ),
    d(
        "bynk.service.websocket_header",
        "The `from websocket` header is malformed — it binds frame types as `websocket(in: <type>, out: <type>)` (real-time track slice 3).",
    ),
    d(
        "bynk.service.websocket_multiple",
        "A context holds more than one `from websocket` service — at v1 the Workers upgrade routes by the `Upgrade: websocket` header alone, so one WebSocket service per context (real-time track slice 3b).",
    ),
    d(
        "bynk.service.websocket_open_arity",
        "A `from websocket` service must hold exactly one `on open` handler (the edge upgrade), and at most one `on message` (inbound) and one `on close` (real-time track slice 3/3b-iii).",
    ),
    d(
        "bynk.service.websocket_param_not_stringy",
        "A `from websocket` service's `on open` parameter has a type not constructible from `String`; it arrives as a query-string value (#1657).",
    ),
    d(
        "bynk.stability.experimental",
        "An experimental standard-library item is used without an `[unstable] allow` entry in `bynk.toml` covering it.",
    ),
    d(
        "bynk.store.annotation_kind_mismatch",
        "A storage annotation is used on a kind it does not apply to (e.g. `@ttl` on a `Map`).",
    ),
    d(
        "bynk.store.annotation_unsupported",
        "A known storage annotation that nothing acts on yet (`@bounded`) is used; the others (`@ttl`, `@retain`, `@indexed`) are functional.",
    ),
    d(
        "bynk.store.cache_needs_clock",
        "A handler performs a `Cache` operation (TTL expiry reads the clock) without declaring `given Clock`.",
    ),
    d(
        "bynk.store.cache_ttl_required",
        "A `Cache` field is missing its required `@ttl(<duration>)` annotation (a keyed store with no expiry is a `Map`).",
    ),
    d(
        "bynk.store.kind_arity",
        "A storage kind was applied to the wrong number of type arguments (e.g. `Cell[A, B]`).",
    ),
    d(
        "bynk.store.kind_unsupported",
        "A known storage kind (`Queue`) is used before the slice that supports it.",
    ),
    d(
        "bynk.store.log_needs_clock",
        "A handler calls `Log.append` (which stamps the current time) without declaring `given Clock`.",
    ),
    d(
        "bynk.store.unkeyable_key",
        "A store `Set` element or `Map`/`Cache` key is not value-keyable — it must be `String`, `Int`, or a refined/opaque type over them (#1680).",
    ),
    d(
        "bynk.store.unknown_annotation",
        "A `store` field carries an annotation outside the closed `@indexed`/`@ttl`/`@retain`/`@bounded` set.",
    ),
    d(
        "bynk.store.unknown_kind",
        "A `store` field's type is not a known storage kind.",
    ),
    d(
        "bynk.store.unknown_map_accessor",
        "A `store Map` field access is not one of its query accessors (`entries`/`keys`/`values`).",
    ),
    d(
        "bynk.store.unknown_op",
        "A method called on a `store` field (`Map`, `Cache`, `Set`, `Cell` or `Log`) is not one of that kind's storage operations.",
    ),
    d(
        "bynk.stub.after_type",
        "A test `stub … returns <value> after <d>` latency is not a `Duration`.",
    ),
    dg(
        "bynk.stub.at_system",
        "A test `stub` is in a `system`-tier case, or at the scope of a `suite … as system` (or a suite whose cases run `as system`); each context runs as the Worker it deploys as, which builds its own capability providers, so there is no seam to override.",
        &["stub_clause"],
    ),
    d(
        "bynk.stub.bad_sequence",
        "A `stub … returns each […]` sequence is malformed (e.g. empty).",
    ),
    d(
        "bynk.stub.generic_op",
        "A test `stub` targets a capability operation that declares its own type parameter — not supported at v1.",
    ),
    d(
        "bynk.stub.not_a_seam",
        "A test `stub` overrides a capability the unit under test does not consume.",
    ),
    d(
        "bynk.stub.rhs_type",
        "A test `stub … returns <value>` right-hand side does not match the operation's return type.",
    ),
    d(
        "bynk.stub.unknown_op",
        "A test `stub` names an operation the capability does not declare.",
    ),
    dg(
        "bynk.suite.const_name_clash",
        "A suite `const` has the name of a declaration or sum variant already in scope in the suite's cases.",
        &["const_decl"],
    ),
    dg(
        "bynk.suite.duplicate_case_name",
        "Two `case`s share a description.",
        &["case"],
    ),
    dg(
        "bynk.suite.duplicate_const",
        "A suite declares two `const`s of the same name, or a `const` with the name of one of its `type`s.",
        &["const_decl"],
    ),
    dg(
        "bynk.suite.duplicate_env",
        "An `env { … }` clause gives the same name twice at one scope (a suite's `env` clauses, or one case's); a case's `env` may override a suite's name, but not repeat its own.",
        &["env_clause"],
    ),
    dg(
        "bynk.suite.duplicate_fn",
        "A suite declares two helper `fn`s of the same name.",
        &["fn_decl"],
    ),
    dg(
        "bynk.suite.duplicate_type",
        "A suite declares two `type`s of the same name.",
        &["type_decl"],
    ),
    dg(
        "bynk.suite.env_binding_clash",
        "A `system`-tier suite's or case's `env` names a binding a participant's Worker already declares (a Service Binding, KV namespace, or Durable Object namespace), or a key the test harness sets on it (a call-edge key, a `Foreign` caller's secret), so the entry would clash with it on the Worker's `env`.",
        &["env_clause"],
    ),
    dg(
        "bynk.suite.env_not_literal",
        "An `env { … }` entry's value is neither a `String` literal nor a `fixture` the same suite declares; a Worker's variables and secrets are strings, given verbatim.",
        &["env_clause"],
    ),
    dg(
        "bynk.suite.fn_effectful",
        "A suite helper `fn` returns an `Effect`; suite helpers are pure.",
        &["fn_decl"],
    ),
    dg(
        "bynk.suite.fn_method",
        "A suite helper `fn` is a method (`fn Type.name`); suite helpers are free functions.",
        &["fn_decl"],
    ),
    dg(
        "bynk.suite.fn_name_clash",
        "A suite helper `fn` has the name of a declaration already in scope in the suite's cases.",
        &["fn_decl"],
    ),
    dg(
        "bynk.suite.type_generic",
        "A suite declares a generic `type`; a suite type is a plain, non-generic shape.",
        &["type_decl"],
    ),
    dg(
        "bynk.suite.type_name_clash",
        "A suite `type`, or a variant of one, has the name of a declaration or sum variant already in scope in the suite's cases, or of another suite item.",
        &["type_decl"],
    ),
    dg(
        "bynk.suite.unknown_target",
        "A `suite` targets a unit that does not exist.",
        &["suite_decl"],
    ),
    d(
        "bynk.target.browser_bundle_only",
        "The `browser` platform builds only the in-process `Bundle` topology; `--target workers` is not a browser build.",
    ),
    dg(
        "bynk.target.vendor_conflict",
        "One deployment unit's in-process closure uses platform-native capabilities from two mutually-exclusive platforms.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.target.vendor_required",
        "A deployment unit uses a platform-native capability but the build selects another `--platform`.",
        &["consumes_decl"],
    ),
    dg(
        "bynk.test.actor_identity_required",
        "A test omits the identity an identity-carrying actor requires: a bare call-site `by <Actor>`, or no `by` at all on a handler that binds its `Caller`.",
        &["case"],
    ),
    dg(
        "bynk.test.actor_no_identity",
        "A call-site `by <Actor>(x)` supplies an identity to an actor that takes none — a unit-identity actor (e.g. `Visitor`) or `Nobody`.",
        &["case"],
    ),
    dg(
        "bynk.test.advance_not_clock",
        "An `advance` statement names a capability other than `Clock`; the test harness owns one time source, the first-party `Clock`.",
        &["advance_stmt"],
    ),
    dg(
        "bynk.test.advance_not_duration",
        "An `advance Clock by` statement's amount is not a `Duration`.",
        &["advance_stmt"],
    ),
    dg(
        "bynk.test.advance_outside_case",
        "An `advance Clock by` statement appears outside a test `case` body; only the test harness has a clock to move.",
        &["advance_stmt"],
    ),
    dg(
        "bynk.test.asset_address_method",
        "A suite fetches an asset with a method other than `GET`; the platform serves assets to `GET` and answers other methods with 405.",
        &["case"],
    ),
    dg(
        "bynk.test.call_needs_in_process",
        "A `system`-tier case calls an `on call` service with `svc.call(…)`; an `on call` service is internal to a deployment and has no public route for a `system` case to drive, so the case drives an `http` route that reaches it, or calls it at `unit` or `integration`.",
        &["case"],
    ),
    dg(
        "bynk.test.credential_needs_system",
        "A case drives `by Nobody` (the no-credential principal, which tests the auth seam's 401) outside a `system`-tier case, where there is no real seam to reject it.",
        &["case"],
    ),
    dg(
        "bynk.test.event_needs_in_process",
        "A `system`-tier case delivers an event with `svc.event(e)`; at `system` an event reaches a subscriber only through its publisher's fan-out, so the case drives the emitting handler instead, or delivers the event at `unit` or `integration`.",
        &["case"],
    ),
    dg(
        "bynk.test.event_not_subscribed",
        "A test body's `svc.event(e)` delivers an event the `from Events(E)` service does not subscribe to.",
        &["case"],
    ),
    dg(
        "bynk.test.event_publisher_ambiguous",
        "A test body's `svc.event(e)` omits the envelope, but the event's publisher, which the default envelope names, is not one context the target declares or consumes.",
        &["case"],
    ),
    dg(
        "bynk.test.expired_needs_bearer_route",
        "A case drives `by Expired` at an address that is not an http route with a `Bearer` actor (alone, or as a member of an actor sum) or an `Oidc` actor; it sends a correctly signed token whose `exp` has passed, so only a `Bearer` or `Oidc` seam can refuse it.",
        &["case"],
    ),
    dg(
        "bynk.test.extra_headers_authorization",
        "A `system` case sends `Authorization` as an extra request header to a route that declares no `headers:` parameter; the case's `by <Actor>` owns the credential the driver sends.",
        &["case"],
    ),
    dg(
        "bynk.test.faults_needs_in_process",
        "A `system`-tier case claims a fault with `expect <call> faults`, or a fault claim at any tier is driven `by Nobody`, `by Forged`, `by Stale`, `by Expired`, `by WrongIssuer`, `by WrongAudience`, `by WrongAlgorithm` or `by NoSubject`; at `system` a fault reaches the case as an error response from the deployed Worker, never a throw, so the claim belongs at `unit` or `integration`.",
        &["case"],
    ),
    dg(
        "bynk.test.forged_needs_signature_route",
        "A case drives `by Forged` at an address that is not an http route with a `Signature`, `Bearer` or `Oidc` actor, or `by Stale` at one with no `Signature` actor (alone, or as a member of an actor sum); they send a bad credential, so only a `Signature`, `Bearer` or `Oidc` seam can refuse them.",
        &["case"],
    ),
    dg(
        "bynk.test.forged_needs_system",
        "A case drives `by Forged`, `by Stale`, `by Expired`, `by WrongIssuer`, `by WrongAudience`, `by WrongAlgorithm` or `by NoSubject` outside a `system`-tier case; only the `system` driver signs a request or mints a token, so only there does the real `Signature`, `Bearer` or `Oidc` seam see the bad credential.",
        &["case"],
    ),
    dg(
        "bynk.test.no_subject_needs_oidc_route",
        "A case drives `by NoSubject` at an address that is not an http route with an `Oidc` actor; it sends correctly signed tokens whose `sub` is empty and whose `sub` is missing, so only an `Oidc` seam can refuse them.",
        &["case"],
    ),
    dg(
        "bynk.test.nobody_needs_secured_route",
        "A case drives `by Nobody` at a route with no Bearer, `Oidc` or Signature actor (alone, or as a member of an actor sum) (e.g. a public `Visitor` route) — there is no auth seam to reject the missing credential.",
        &["case"],
    ),
    d(
        "bynk.test.principal_claims_not_oidc",
        "A call-site principal passes token claims for an actor that isn't `Oidc`.",
    ),
    dg(
        "bynk.test.principal_identity_mismatch",
        "A call-site `by <Actor>` acts as an actor whose identity is incompatible with the addressed handler's actor, or names no member of the handler's actor sum.",
        &["case"],
    ),
    dg(
        "bynk.test.principal_on_asset",
        "A suite's asset fetch carries a `by` clause; an asset is public and served before the Worker runs.",
        &["case"],
    ),
    dg(
        "bynk.test.principal_on_wrong_method",
        "A wrong-method `405` test carries a `by <Actor>` clause; it reaches no handler, so a principal is meaningless.",
        &["case"],
    ),
    dg(
        "bynk.test.principal_required",
        "A test drives an identity-carrying handler, or one guarded by an actor sum, with no call-site `by <Actor>`.",
        &["case"],
    ),
    warn(dg(
        "bynk.test.remote_clock_dependent",
        "Under `bynk test --against`, a `system` case's `expect` compares an `Instant` that isn't derived from the run (a constant such as `Instant.fromEpochMillis(0)`); in process the clock starts at the epoch, but a real runtime's Workers read wall-clock time.",
        &["case"],
    )),
    dg(
        "bynk.test.remote_unsupported_advance",
        "A `system` case moves the harness clock with `advance Clock`, which a run against a real runtime (`bynk test --against`) can't honour: the deployed Workers read wall-clock time.",
        &["case"],
    ),
    dg(
        "bynk.test.remote_unsupported_asset",
        "A `system` case fetches an asset with `svc.GET(asset)`, which a run against a real runtime (`bynk test --against`) can't honour: the fetch is answered in process.",
        &["case"],
    ),
    dg(
        "bynk.test.remote_unsupported_call",
        "A `system` case addresses an `on call` service, which a run against a real runtime (`bynk test --against`) can't honour: the service is internal, and each call is signed for one caller.",
        &["case"],
    ),
    dg(
        "bynk.test.remote_unsupported_env",
        "A `system` case's `env` disagrees with the named target it would run against (`bynk test --against`): it sets a secret, sets a name the target doesn't declare under `vars` or declares with another value, or leaves unset a name the target declares.",
        &["case"],
    ),
    dg(
        "bynk.test.remote_unsupported_queue",
        "A `system` case delivers a queue message with `q.message(m)`, which a run against a real runtime (`bynk test --against`) can't honour: no public wire reaches a queue consumer.",
        &["case"],
    ),
    dg(
        "bynk.test.remote_unsupported_schedule",
        "A `system` case runs a cron handler with `svc.schedule(…)`, which a run against a real runtime (`bynk test --against`) can't honour: no public wire reaches a cron handler.",
        &["case"],
    ),
    dg(
        "bynk.test.service_bad_address",
        "A test body addresses a service the wrong way for its protocol (e.g. an http route without a leading path string).",
        &["case"],
    ),
    dg(
        "bynk.test.service_call_arity",
        "A test body's `svc.call(...)` passes the wrong number of arguments for the service's `on call` handler.",
        &["case"],
    ),
    dg(
        "bynk.test.service_no_call_handler",
        "A test body invokes `svc.call(...)` on a service with no `on call` handler (a `from http`/`cron`/`queue` service).",
        &["case"],
    ),
    dg(
        "bynk.test.service_unknown_route",
        "A test body addresses an http route / cron schedule / queue message the service does not declare.",
        &["case"],
    ),
    warn(dg(
        "bynk.test.smoke_undeclared_status",
        "A smoke case (`bynk test --smoke`) gets a non-5xx answer whose status is not among those its route's handler declares; the case still passes (warning).",
        &["case"],
    )),
    dg(
        "bynk.test.snapshot_duplicate_name",
        "A case takes two snapshots (`expect v matches snapshot \"name\"`) under one name; a snapshot is keyed by its case and name.",
        &["snapshot_expr"],
    ),
    dg(
        "bynk.test.snapshot_in_property",
        "A `property` takes a snapshot; its body runs once per generated input, so the snapshot belongs in a `case` over a fixed value.",
        &["snapshot_expr"],
    ),
    dg(
        "bynk.test.snapshot_name_not_literal",
        "`matches snapshot` is not followed by a string literal; a snapshot is named by a literal so the compiler knows every snapshot a suite takes.",
        &["snapshot_expr"],
    ),
    dg(
        "bynk.test.snapshot_needs_unit_suite",
        "A snapshot is taken in a `system`-tier case or in an integration or system suite; snapshots are stored per unit suite file and compared in the unit harness.",
        &["snapshot_expr"],
    ),
    dg(
        "bynk.test.snapshot_not_codable",
        "The value of an `expect … matches snapshot` is not `Codable`: a snapshot stores the value's `Json.encode` text, so it must be in the domain `Json.encode` accepts.",
        &["snapshot_expr"],
    ),
    dg(
        "bynk.test.stale_needs_timestamp",
        "A case drives `by Stale` at a `Signature` route whose actor declares no `timestamp` header and `tolerance`, so there is no replay window for the seam to refuse a stale signature by.",
        &["case"],
    ),
    dg(
        "bynk.test.system_claims_need_literal",
        "A `system`-tier case passes token claims to a call-site principal other than as a literal `TokenClaims.of([…])`; the driver mints the `Oidc` token's claims from those lines, which a `TokenClaims` value does not expose.",
        &["case"],
    ),
    dg(
        "bynk.test.unknown_actor",
        "A call-site `by <Actor>` names an actor the target context does not declare and that is not a prelude actor.",
        &["case"],
    ),
    dg(
        "bynk.test.wire_http_only",
        "A `Wire(...)` raw argument is passed to a queue, cron, `on call` or event address; `Wire` drives an http body or path segment only, so a queue message, cron tick or event is passed as a typed value.",
        &["case"],
    ),
    dg(
        "bynk.test.wire_needs_system",
        "A `Wire(...)` raw argument is used outside a `system`-tier service address; `Wire` hands pre-validation input to the boundary and is meaningless at `unit` or in any other position.",
        &["case"],
    ),
    dg(
        "bynk.test.wrong_algorithm_needs_oidc_route",
        "A case drives `by WrongAlgorithm` at an address that is not an http route with an `Oidc` actor; it sends a token with `alg: none` and an `HS256` token keyed with the JWKS public key, so only an `Oidc` seam can refuse them.",
        &["case"],
    ),
    dg(
        "bynk.test.wrong_audience_needs_oidc_route",
        "A case drives `by WrongAudience` at an address that is not an http route with an `Oidc` actor; it sends a correctly signed token whose `aud` is not the actor's, so only an `Oidc` seam can refuse it.",
        &["case"],
    ),
    dg(
        "bynk.test.wrong_issuer_needs_oidc_route",
        "A case drives `by WrongIssuer` at an address that is not an http route with an `Oidc` actor; it sends a correctly signed token whose `iss` is not the actor's, so only an `Oidc` seam can refuse it.",
        &["case"],
    ),
    dg(
        "bynk.test.ws_lifecycle_by",
        "A case's `svc.message(tc, frame)` or `svc.close(tc)` on a `from websocket` service carries a `by` clause; the connection's principal is given once, at `open`, and carried from there.",
        &["case"],
    ),
    dg(
        "bynk.test.ws_needs_in_process",
        "A `system`-tier case drives a `from websocket` service by address (`svc.open(tc, …)`, `svc.message(tc, frame)`, `svc.close(tc)`); a deployed WebSocket is a real upgrade through the Worker and a Durable Object, which a `system` case does not drive, so the case runs at `unit` or `integration`.",
        &["case"],
    ),
    dg(
        "bynk.tier.cross_context_needs_system",
        "A `unit` or `integration` case, or a `property`, reaches another context's service, directly or through a target service or agent handler that calls one; below `system` no other context is stood up to call, so the flow belongs in a `suite … as system`.",
        &["case"],
    ),
    dg(
        "bynk.tier.mixed_system_suite",
        "A suite mixes `system` cases with `unit` or `integration` ones; a `system` case runs against deployed Workers and addresses services by context path, so it needs a `suite … as system` of its own.",
        &["case"],
    ),
    d(
        "bynk.tier.system_needs_wire",
        "An `as system` suite whose target has no serialisation edge: it consumes no other context and exposes no `http`, `queue` or `email` service, so there is no wire for a `system` case to cross.",
    ),
    d(
        "bynk.transition.cross_agent_reference",
        "A transition predicate references another agent; step invariants are per-agent.",
    ),
    d(
        "bynk.transition.duplicate_name",
        "An agent declares two transitions with the same name.",
    ),
    d(
        "bynk.transition.impure_predicate",
        "A transition predicate uses an effectful or test-only construct; a step invariant must be pure.",
    ),
    warn(d(
        "bynk.transition.may_violate",
        "A handler can make a move that an agent's `transition` refuses, from a state the agent can be in after a commit; at run time that call faults with `InvariantViolation` (warning). Resolve it in code: refuse the move first, with a `match` arm or an `is` test on the state field that the path can't get past.",
    )),
    d(
        "bynk.transition.no_step_reference",
        "A transition references neither `old` nor `new`; it constrains one state, so it is an `invariant`, not a step.",
    ),
    d(
        "bynk.transition.not_bool",
        "A transition predicate does not have type `Bool`.",
    ),
    dg(
        "bynk.types.argument_mismatch",
        "A call, method, capability, or constructor argument has the wrong type.",
        &["call"],
    ),
    d(
        "bynk.types.asset_at_boundary",
        "An `Asset` type appears in a record field, boundary or store position; pass its `.url` instead.",
    ),
    d(
        "bynk.types.bad_field_default",
        "A record or event field's default expression (`field: T = expr`) is not a static, wire-representable value of the field's declared type — a literal (including one admitted to a refined type), a sum variant of a type this context may construct (not a `uses`-sourced commons sum, which is `bynk.context.rebrand_construction`), `Some`/`None`/`Ok`/`Err`, a record, a list literal, `Map.empty()`/`Set.empty()`, or `T.unsafe(lit)` for an opaque type whose literal also satisfies the refinement. A field whose type is a generic record's type parameter has no default, and a default may not hold for one instantiation only: for a record field that applies a user generic (`Page[String]`, or `Page[T]` inside a generic record; an event or old-state field may not) it is a closed-form value at the instantiated type, writing nothing of the enclosing record's own type parameters. (Earlier versions reported this as `bynk.event.bad_field_default`.)",
    ),
    dg(
        "bynk.types.call_arity",
        "A function value, a built-in `Map`/`Set`/`Cell`/`Cache`/`Log` operation, or `unreachable`/`todo` was called with the wrong number of arguments (a declared `fn` is `bynk.resolve.arity_mismatch`).",
        &["call"],
    ),
    dg(
        "bynk.types.cannot_infer_option_type_param",
        "The value type of `None` could not be inferred.",
        &["none_expr"],
    ),
    d(
        "bynk.types.cannot_infer_result_type_params",
        "The type parameters of a `Result` could not be inferred.",
    ),
    dg(
        "bynk.types.catastrophic_regex",
        "A `Matches` pattern, or a regex method's pattern, can match the same text in exponentially many ways (nested or ambiguous repetition, bounded or not), or cannot be analysed, risking catastrophic backtracking (ReDoS).",
        &["refinement"],
    ),
    d(
        "bynk.types.claims_at_boundary",
        "`TokenClaims` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent.",
    ),
    dg(
        "bynk.types.combinator_return_mismatch",
        "A callback passed to a combinator (`map`/`andThen`/`flatMap`/`traverseAll`/…) returns the wrong type.",
        &["call"],
    ),
    d(
        "bynk.types.constructor_arity",
        "`T.of` (or `T.unsafe`) on a refined or opaque type got the wrong number of arguments.",
    ),
    d(
        "bynk.types.constructor_base_mismatch",
        "A `.of` constructor was given an argument of the wrong base type.",
    ),
    d(
        "bynk.types.decimal_division_unscaled",
        "`/` was used on `Decimal` operands. A quotient has no exact decimal answer, so division states its scale and rounding mode: `a.div(b, 2, RoundingMode.HalfEven)` (#2622).",
    ),
    d(
        "bynk.types.decimal_negation_too_large",
        "A negated decimal literal would have more than 1000 characters of canonical text once its sign is counted, the most any `Decimal` may have (#2622).",
    ),
    warn(d(
        "bynk.types.deprecated_json_error",
        "`JsonError` is the deprecated spelling of `DecodeError` (warning; auto-fixable).",
    )),
    dg(
        "bynk.types.derives_automatic",
        "A `derives` clause names `Eq`, `Key` or `Codable`, which hold for a type automatically; only `Ord` is opted into. The diagnostic carries a machine-applicable fix that removes the name (constrained-generics slice 1).",
        &["type_decl"],
    ),
    dg(
        "bynk.types.derives_field_not_orderable",
        "A type declares `derives Ord`, but one of its fields or payloads is not `Ord` (for example a `Map`, a `Set`, a `Bool`, a function, or a record that does not itself derive `Ord`); every part must be orderable for the declaration order to be a total order (constrained-generics slice 1).",
        &["type_decl"],
    ),
    dg(
        "bynk.types.duplicate_example",
        "A refinement carries more than one `@example`.",
        &["refinement"],
    ),
    dg(
        "bynk.types.duplicate_literal_arm",
        "A `match` has two arms for the same literal value.",
        &["match_arm"],
    ),
    dg(
        "bynk.types.duplicate_pattern_field",
        "A record pattern lists the same field twice.",
        &["record_pattern"],
    ),
    dg(
        "bynk.types.duplicate_variant_arm",
        "A `match` has two arms for the same variant.",
        &["match_arm"],
    ),
    d(
        "bynk.types.embeds_ambiguous",
        "A type is embedded by more than one variant of a sum, so `?`'s conversion would be ambiguous.",
    ),
    d(
        "bynk.types.embeds_unknown_variant",
        "An `embeds … as V` clause names a variant the sum does not declare.",
    ),
    d(
        "bynk.types.embeds_variant_shape",
        "An `embeds E as V` target variant must have exactly one payload field, of type `E`.",
    ),
    dg(
        "bynk.types.empty_range_pattern",
        "A range pattern or `via schema(...)` range has its bounds inverted (`lo > hi`), so it can never match.",
        &["int_range"],
    ),
    dg(
        "bynk.types.empty_refinement",
        "A refinement admits no values (contradictory predicates).",
        &["refinement"],
    ),
    dg(
        "bynk.types.err_value_mismatch",
        "An `Err` payload has the wrong type.",
        &["err_expr"],
    ),
    dg(
        "bynk.types.example_not_literal",
        "An `@example` is not exactly one literal of the base type.",
        &["refinement"],
    ),
    dg(
        "bynk.types.example_on_named_field",
        "An `@example` on a field whose type is a named type (it belongs on the named type).",
        &["refinement"],
    ),
    dg(
        "bynk.types.example_unneeded",
        "An `@example` on a refinement `Val` can already generate (only `Matches` and `IsoDate` take one).",
        &["refinement"],
    ),
    dg(
        "bynk.types.example_violates",
        "An `@example` does not satisfy its refinement's predicates.",
        &["refinement"],
    ),
    d(
        "bynk.types.extensible_match_needs_wildcard",
        "A `match` covers an extensible standard-library enum (`FetchError`, `Method`) without a wildcard; it may gain variants in a minor release.",
    ),
    dg(
        "bynk.types.field_access_on_non_record",
        "Field access on a value that is not a record.",
        &["field_access"],
    ),
    dg(
        "bynk.types.field_annotation_unknown",
        "A record field carries an annotation other than `@name(\"…\")`.",
        &["record_field"],
    ),
    d(
        "bynk.types.field_default_on_variant",
        "A sum variant's payload field declares a default (`field: T = expr`). Only a record's fields may; a variant is built like a call, with every payload field given.",
    ),
    dg(
        "bynk.types.field_name_bad_arg",
        "A field's `@name` does not take exactly one string literal, or a `String` constant bound to one.",
        &["record_field"],
    ),
    dg(
        "bynk.types.field_name_case_conflict",
        "Two fields of a `cookies` or `query` record have wire names that differ only in case.",
        &["record_field"],
    ),
    dg(
        "bynk.types.field_name_duplicate",
        "A field has more than one `@name`, or two fields of a request record read the same wire name.",
        &["record_field"],
    ),
    dg(
        "bynk.types.field_name_invalid",
        "A field's `@name` is not a valid name for where the record is read (a cookie name that is not a token, or an empty name).",
        &["record_field"],
    ),
    dg(
        "bynk.types.field_name_not_request_record",
        "`@name` is on a field of a record that is not the `cookies`, `query` or `@form`/`@multipart` `body` record of an HTTP handler.",
        &["record_field"],
    ),
    dg(
        "bynk.types.field_name_record_not_boundary",
        "A record with `@name` fields is used as a JSON body, a result, a store field, an event or a call argument.",
        &["record_field"],
    ),
    dg(
        "bynk.types.field_refinement_not_base",
        "An inline field refinement requires a base or refined type.",
        &["record_field"],
    ),
    dg(
        "bynk.types.field_value_mismatch",
        "A record field was given a value of the wrong type.",
        &["record_construction"],
    ),
    d(
        "bynk.types.file_at_boundary",
        "A `File` (a file part of a multipart request), or a type carrying one, is used where it would be persisted, sent across a boundary or returned.",
    ),
    d(
        "bynk.types.form_uncodable",
        "`Form.decode` or `Form.encode` names a type that is not a record of form fields (a `String`, `Int`, `Bool`, `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` or a `List`).",
    ),
    dg(
        "bynk.types.function_at_boundary",
        "A function type appeared in a serialisable or boundary position (a record field, sum payload, service/agent handler signature, capability operation signature, agent state field, or agent key); functions cannot serialise or cross a boundary.",
        &["function_type_ref"],
    ),
    dg(
        "bynk.types.guard_not_bool",
        "A match-arm `if` guard is not a `Bool` expression.",
        &["match_arm"],
    ),
    d(
        "bynk.types.headers_at_boundary",
        "`RequestHeaders` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent.",
    ),
    d(
        "bynk.types.held_at_boundary",
        "A held value (`Connection[F]`) appears in a serialisable or boundary position — a held resource is built and disposed in place, never persisted or sent across a boundary (§2.9, real-time track slice 2).",
    ),
    d(
        "bynk.types.held_not_comparable",
        "A held value (`Connection[F]`) is compared with `==`/`!=` — held values have identity, not value-equality (§2.9.3, real-time track slice 2).",
    ),
    dg(
        "bynk.types.if_branch_mismatch",
        "The branches of an `if` have different types.",
        &["if_expr"],
    ),
    dg(
        "bynk.types.if_non_bool_cond",
        "An `if` condition is not a `Bool`.",
        &["if_expr"],
    ),
    dg(
        "bynk.types.if_without_else_requires_unit",
        "An `if` with no `else` branch has a non-unit then-branch; the missing else defaults to `()`, so the branch must be `()` or `Effect[()]`.",
        &["if_expr"],
    ),
    d(
        "bynk.types.interpolation_non_scalar",
        "An interpolation hole holds a value with no string form.",
    ),
    dg(
        "bynk.types.invalid_regex",
        "A `Matches` predicate, or a regex method's pattern (`containsRegex`, …), contains an invalid regular expression or malformed inline flags.",
        &["refinement"],
    ),
    dg(
        "bynk.types.inverted_range",
        "An `InRange` predicate has its bounds inverted.",
        &["refinement"],
    ),
    dg(
        "bynk.types.is_base_mismatch",
        "An `is` refinement check is applied to a value of the wrong base type.",
        &["is_expr"],
    ),
    dg(
        "bynk.types.is_literal_pattern",
        "A literal was used on the right of `is`; `is` tests type/refinement, not value equality (use `==`).",
        &["is_expr"],
    ),
    dg(
        "bynk.types.is_non_sum",
        "`is` was applied to a value that is not a sum type.",
        &["is_expr"],
    ),
    dg(
        "bynk.types.is_range_pattern",
        "A range pattern was used directly on the right of `is`; `is` tests type/refinement, not value ranges (use `match`, a guard or `InRange`).",
        &["is_expr"],
    ),
    dg(
        "bynk.types.is_refined_pattern",
        "A refined (`where`) pattern was used on the right of `is`; refined patterns are `match`-only.",
        &["is_expr"],
    ),
    dg(
        "bynk.types.is_unknown_variant",
        "`is` names a variant the type does not have.",
        &["is_expr"],
    ),
    dg(
        "bynk.types.json_uncodable",
        "A `Json.encode`/`Json.decode` target type cannot pass through the typed JSON codec (functions, effects, streams or a type that carries one, `bynk.crypto` keys, error builtins).",
        &["method_call"],
    ),
    d(
        "bynk.types.key_at_boundary",
        "A `bynk.crypto` `SigningKey` or `VerifyKey` appears in a storable or boundary-crossing position — a key is held non-extractable and lives for the call that imported it, never persisted, logged or sent (standard-library K4).",
    ),
    dg(
        "bynk.types.key_not_orderable",
        "A `sortBy`/`sortByDescending`/`min`/`max` key function does not return an orderable type. The key must be an `Ord` type, on a `List` and on a storage `Query` alike.",
        &[],
    ),
    dg(
        "bynk.types.lambda_mismatch",
        "A lambda's parameter count, parameter annotations, or body type do not match the expected function type.",
        &["lambda_expr"],
    ),
    dg(
        "bynk.types.let_annotation_mismatch",
        "A `let` value does not match its type annotation.",
        &["let_stmt"],
    ),
    dg(
        "bynk.types.list_element_mismatch",
        "A list-literal element has a different type from the list's element type.",
        &["list_literal"],
    ),
    dg(
        "bynk.types.list_pattern_on_non_list",
        "A list pattern is matched against a value that is not a `List` (a record, sum, base type, `Set` or `Map`).",
        &["list_pattern"],
    ),
    dg(
        "bynk.types.match_arm_mismatch",
        "A `match` arm has a different type from the others.",
        &["match_arm"],
    ),
    dg(
        "bynk.types.match_non_sum_discriminant",
        "`match` was applied to a value that is not a sum type.",
        &["match_expr"],
    ),
    dg(
        "bynk.types.method_arity",
        "A method was called with the wrong number of arguments.",
        &["method_call"],
    ),
    dg(
        "bynk.types.method_not_found",
        "Called a method the type does not have.",
        &["method_call"],
    ),
    dg(
        "bynk.types.method_on_non_named_type",
        "A method was called on a built-in type that has no methods.",
        &["method_call"],
    ),
    dg(
        "bynk.types.mixed_pattern_bindings",
        "A pattern mixes named and positional bindings.",
        &["variant_pattern"],
    ),
    dg(
        "bynk.types.negative_length",
        "A length predicate was given a negative value.",
        &["refinement"],
    ),
    dg(
        "bynk.types.never_inferred",
        "An unannotated `let` or `<-` binding has a type that is, or contains, the bottom type (`never returns`) because its value only faults (a faulting lambda bound this way included); the type cannot be written in source, so an annotation is needed.",
        &["let_stmt"],
    ),
    dg(
        "bynk.types.no_numeric_coercion",
        "`Int`, `Float` and `Decimal` were mixed without an explicit conversion — in an operation, a comparison or refinement bounds.",
        &["binary_expr", "refinement"],
    ),
    dg(
        "bynk.types.non_exhaustive_match",
        "A `match` does not cover every value.",
        &["match_expr"],
    ),
    d(
        "bynk.types.not_codable",
        "A type is instantiated at a `Codable`-bounded parameter (of a generic function, or of a capability operation) but is outside the domain `Json.decode` accepts: top-level `()`, a function, `Effect`, `Query`, a `Stream` or a type that carries one, a connection, the HTTP and queue result types, `Asset`, request headers, token claims, or an actor (constrained-generics slice 3).",
    ),
    d(
        "bynk.types.not_comparable",
        "A value compared with `==`/`!=` contains a function, `Effect`, or `Query` somewhere inside it — those have no value equality, so the type is not equality-supporting (type-system §2.3.5, #1652).",
    ),
    dg(
        "bynk.types.not_keyable",
        "A `Map` key, `Set` element, or `distinct`/`distinctBy`/`groupBy`/join key, or an argument to a `Key`-bounded parameter, is not `Key`. `String`, `Int`, `Bool`, `Instant`, `Duration`, `Bytes`, `Option`/`List` of a key type, refined and opaque types over them, and records, sums and enums made only of key types are; a `Float` is never one, at any depth, because `NaN` is not equal to itself (constrained-generics slice 2).",
        &["generic_type_ref"],
    ),
    d(
        "bynk.types.not_orderable",
        "A value is ordered (`<`, `<=`, `>`, `>=`), or passed to an `Ord`-bounded parameter, but its type is not `Ord`. `Int`, `Float`, `String`, `Duration`, `Instant`, `Bytes`, `Option`/`List` of an orderable type, refined types over those, and types that declare `derives Ord` are; adding `derives Ord` is offered as a fix that is never applied automatically, because it makes declaration order significant (constrained-generics slice 1).",
    ),
    dg(
        "bynk.types.ok_value_mismatch",
        "An `Ok` payload has the wrong type.",
        &["ok_expr"],
    ),
    dg(
        "bynk.types.opaque_raw_outside",
        "`.raw` on an opaque type was used outside its defining commons.",
        &["field_access"],
    ),
    dg(
        "bynk.types.opaque_unsafe_outside",
        "`.unsafe` on an opaque type was used outside its defining context.",
        &["field_access"],
    ),
    dg(
        "bynk.types.or_pattern_binding_mismatch",
        "An or-pattern's alternatives don't all bind the same set of names.",
        &["match_arm", "is_expr"],
    ),
    dg(
        "bynk.types.or_pattern_type_mismatch",
        "An or-pattern's alternatives give a shared binding different types (or refinements).",
        &["match_arm", "is_expr"],
    ),
    dg(
        "bynk.types.pattern_arity",
        "A pattern binds the wrong number of payload fields.",
        &["variant_pattern"],
    ),
    dg(
        "bynk.types.pattern_type_mismatch",
        "A pattern's type does not match the matched value.",
        &["variant_pattern"],
    ),
    dg(
        "bynk.types.polynomial_regex",
        "A `Matches` pattern can split the same text between repetitions, so matching time grows polynomially with input length, and no length predicate bounds the input small enough for its degree; a regex method's pattern has no bound to excuse it.",
        &["refinement"],
    ),
    warn(dg(
        "bynk.types.polynomial_regex_capped",
        "A `Matches` pattern's matching time grows polynomially with input length, but a `MaxLength` or `Length` predicate bounds the input small enough for its degree (warning).",
        &["refinement"],
    )),
    dg(
        "bynk.types.predicate_base_mismatch",
        "A predicate does not apply to the type's base (e.g. a string predicate on an `Int`).",
        &["refinement"],
    ),
    d(
        "bynk.types.query_at_boundary",
        "A `Query` type appears in a storable or boundary-crossing position — a query is built and executed in place, never persisted or sent (ADR 0115).",
    ),
    dg(
        "bynk.types.question_error_mismatch",
        "`?` propagates an error type incompatible with the function's.",
        &["question_expr"],
    ),
    dg(
        "bynk.types.question_on_non_result",
        "`?` was applied to a non-`Result` value.",
        &["question_expr"],
    ),
    dg(
        "bynk.types.question_option_outside_http",
        "`?` lifts an `Option` only inside a handler returning `HttpResult` (`None` becomes `NotFound`); elsewhere use `.okOr(err)`.",
        &["question_expr"],
    ),
    dg(
        "bynk.types.question_outside_result",
        "`?` used in a function that does not return a `Result`.",
        &["question_expr"],
    ),
    dg(
        "bynk.types.range_pattern_on_non_int",
        "A range pattern (`lo..hi`) was matched against a scrutinee that is not an `Int`.",
        &["int_range"],
    ),
    dg(
        "bynk.types.record_pattern_on_non_record",
        "A record pattern names a type that is not a record (a sum, enum, refined or opaque type, or a built-in).",
        &["record_pattern"],
    ),
    dg(
        "bynk.types.regex_not_literal",
        "A regex method's pattern (`matchesRegex`, `containsRegex`, `findRegex`, `findAllRegex`, `replaceRegex`, `splitRegex`) is not a string literal, so its ReDoS check cannot run at compile time.",
        &["method_call"],
    ),
    d(
        "bynk.types.rejection_at_boundary",
        "`BoundaryRejection` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent.",
    ),
    d(
        "bynk.types.request_cookies_at_boundary",
        "`RequestCookies` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent.",
    ),
    d(
        "bynk.types.request_query_at_boundary",
        "`RequestQuery` appears where a value is stored or sent: a field of a type, or a boundary such as a call, a message or an agent.",
    ),
    d(
        "bynk.types.return_mismatch",
        "A returned value does not match the declared return type.",
    ),
    d(
        "bynk.types.rounding_mode_position",
        "`RoundingMode.<mode>` was used outside the mode argument of a `Decimal`'s `round` or `div`. A mode is read where the rounding happens and is not a first-class value (#2622).",
    ),
    dg(
        "bynk.types.select_tail_has_select",
        "A `D1`, `Postgres` or `MySql` `select[T]` or `firstAs[T]` statement does not start at `FROM` (for example it begins with `SELECT`, `WITH` or a parenthesis); the compiler writes the `SELECT` and column list from `T`.",
        &["method_call"],
    ),
    dg(
        "bynk.types.sequence_needs_result",
        "`sequenceAll`/`sequenceTry` was called on a list whose elements are not `Result`s. They collect a `List[Result[T, E]]`.",
        &["method_call"],
    ),
    dg(
        "bynk.types.some_value_mismatch",
        "A `Some` payload has the wrong type.",
        &["some_expr"],
    ),
    dg(
        "bynk.types.sql_not_literal",
        "A `D1` or `Postgres` statement's SQL (the first argument of `all`, `first`, `run`, `select` or `firstAs`, or the `sql` of a `SqlStatement` or a `PgStatement`) is neither a string literal nor a bare reference to a `String` constant bound to one, so data could be spliced into it instead of bound as a `SqlValue` or `PgValue` parameter (standard-library P4c, Hyperdrive H1 and H3).",
        &["method_call", "record_construction", "record_spread"],
    ),
    dg(
        "bynk.types.sql_row_shape",
        "`Json.decodeSql[T]`'s (or `select[T]`'s, `firstAs[T]`'s) `T` is not a SQL row: a record whose fields are `Int`, `Float`, `String`, `Bool` or `Bytes` (or a refined or opaque type over one, or an `Option` of either), or a `List` of such records (standard-library P4c). A `Postgres` row may also hold a `Decimal` or an `Instant` (Hyperdrive H3).",
        &["method_call"],
    ),
    d(
        "bynk.types.sql_statement_at_boundary",
        "A `bynk.cloudflare` `SqlStatement`, `PgStatement` or `MySqlStatement` appears in a storable or boundary-crossing position, or as a `Json` codec target — a statement from outside would carry SQL that was never a literal (standard-library P4c).",
    ),
    d(
        "bynk.types.stream_at_boundary",
        "A `Stream` type, or a record or sum carrying one in a field, appears in a storable or boundary-crossing position — a stream is a live value-over-time source, never persisted or sent across a boundary (real-time track slice 0; #2623 admits it in capability signatures and record fields).",
    ),
    d(
        "bynk.types.stream_not_comparable",
        "A `Stream` value is compared with `==`/`!=` — a stream is a live value-over-time source, not a comparable value (real-time track slice 0).",
    ),
    d(
        "bynk.types.type_mismatch",
        "An operator or built-in method got an operand of a type it does not accept, or two operands that had to match did not.",
    ),
    dg(
        "bynk.types.uninferable_element_type",
        "An empty `[]` (or `List.empty()` / `Map.empty()`) has no expected type to infer its element type from.",
        &["list_literal"],
    ),
    dg(
        "bynk.types.unknown_annotation",
        "An annotation after a refinement is not `@example`.",
        &["refinement"],
    ),
    dg(
        "bynk.types.unknown_field",
        "Referenced a field the record type does not declare.",
        &["field_access"],
    ),
    dg(
        "bynk.types.unknown_pattern_field",
        "A pattern names a field the variant does not have.",
        &["variant_pattern"],
    ),
    d(
        "bynk.types.unknown_rounding_mode",
        "A `Decimal` `round` or `div` was not given a rounding mode written `RoundingMode.<mode>`; the modes are `Down`, `Up`, `Floor`, `Ceiling`, `HalfUp`, `HalfDown` and `HalfEven` (#2622).",
    ),
    dg(
        "bynk.types.unknown_static_member",
        "A name after `HttpResult.` that is not one of its variants. An unknown member of a type you declared is `bynk.resolve.unknown_static_member`.",
        &["field_access"],
    ),
    dg(
        "bynk.types.unknown_variant_in_pattern",
        "A pattern names a variant the sum type does not have.",
        &["variant_pattern"],
    ),
    dg(
        "bynk.types.unreachable_arm",
        "A `match` arm is unreachable.",
        &["match_arm"],
    ),
    d(
        "bynk.types.variant_arity",
        "A variant constructor got the wrong number of arguments for its payload fields (a pattern with the wrong number of bindings is `bynk.types.pattern_arity`).",
    ),
    d(
        "bynk.types.variant_missing_payload",
        "A variant requiring a payload was used without one.",
    ),
    d(
        "bynk.types.variant_payload_mismatch",
        "A variant payload has the wrong type.",
    ),
    dg(
        "bynk.uses.name_conflict",
        "Two commons a unit `uses` declare the same type, function, constant, asset or embed; or a value crosses between a unit's code and an imported declaration's position whose type the unit shadows; or a reference names an import the unit's own declaration shadows.",
        &["uses_decl"],
    ),
    dg(
        "bynk.uses.self_reference",
        "A commons `uses` itself.",
        &["uses_decl"],
    ),
    dg(
        "bynk.uses.target_is_context",
        "`uses` targets a context instead of a commons.",
        &["uses_decl"],
    ),
    dg(
        "bynk.uses.unknown_commons",
        "`uses` names a commons that does not exist.",
        &["uses_decl"],
    ),
    dg(
        "bynk.val.agent_not_generable",
        "A `for all`/`Val` cannot generate an agent — fabricated agent states need not be reachable.",
        &["for_all"],
    ),
    dg(
        "bynk.val.arity",
        "`Val[T]` was given the wrong number of pin arguments.",
        &["val_expr"],
    ),
    dg(
        "bynk.val.generic_type",
        "A `Val[…]` fabricates a value of a generic type; per-instantiation value fabrication is not yet wired (ADR 0197). The type itself may cross a boundary through its monomorphised codec (v0.174). Named `bynk.generics.generic_record_at_boundary` before #3379.",
        &["val_expr"],
    ),
    dg(
        "bynk.val.literal_violates",
        "A pinned `Val[T]` value violates the type's refinement.",
        &["val_expr"],
    ),
    dg(
        "bynk.val.needs_pin",
        "A bare `Val[T]` cannot generate a value (e.g. a `Matches` string); pin one.",
        &["val_expr"],
    ),
    dg(
        "bynk.val.outside_test",
        "`Val[T]` was used outside a test case body.",
        &["val_expr"],
    ),
    dg(
        "bynk.val.pin_not_literal",
        "A `Val[T]` pin argument is not a compile-time literal.",
        &["val_expr"],
    ),
    dg(
        "bynk.val.pin_unsupported",
        "A pin was given for a type kind that does not support pinning.",
        &["val_expr"],
    ),
    dg(
        "bynk.val.unknown_type",
        "`Val[T]` names a type that does not resolve.",
        &["val_expr"],
    ),
    dg(
        "bynk.val.unsupported_kind",
        "`Val[T]` cannot fabricate a value for this kind of type.",
        &["val_expr"],
    ),
    dg(
        "bynk.visibility.private_in_public",
        "A public declaration's signature names a `private` type of its own unit, which an importer could neither name nor build.",
        &["visibility_modifier"],
    ),
    dg(
        "bynk.visibility.private_item",
        "A reference from another unit names a `private` declaration, which is visible only within its own unit and that unit's suites.",
        &["visibility_modifier"],
    ),
    warn(dg(
        "bynk.visibility.private_messages",
        "A `messages` block in a commons is `private`. A bundle whose `@reference` block is private is never negotiated by a context that uses the commons, and `private` on another block of a bundle whose reference is public has no effect (warning).",
        &["visibility_modifier"],
    )),
    dg(
        "bynk.visibility.private_not_allowed",
        "`private` modifies a declaration it does not apply to: anything but a `fn`, `type`, `messages`, `asset`, `embed` or `const` in a commons or context.",
        &["visibility_modifier"],
    ),
    d(
        "bynk.wasm.panic",
        "The in-browser compiler panicked internally (a compiler bug, reported as a diagnostic instead of crashing the page).",
    ),
    d(
        "bynk.wasm.strip_failed",
        "The in-browser compiler could not strip the emitted TypeScript to JavaScript.",
    ),
    dg(
        "bynk.within.in_agent_handler",
        "`e.within(d)` was used in an agent handler, which has no execution context to hand the abandoned effect to.",
        &["method_call"],
    ),
    dg(
        "bynk.within.in_pure_context",
        "`e.within(d)` was used in a pure (non-effectful) context.",
        &["method_call"],
    ),
    dg(
        "bynk.within.outside_handler",
        "`e.within(d)` was used outside a service handler's body and outside a function (in a provider operation, a WebSocket handler or a test `case` body), where no execution context exists.",
        &["method_call"],
    ),
    dg(
        "bynk.within.timed_out_not_in_scope",
        "`e.within(d)` yields `Result[_, TimedOut]`, but the unit does not consume `bynk`, so `TimedOut` is not in scope.",
        &["method_call"],
    ),
    d(
        "bynk.ws.lifecycle_by_mismatch",
        "A WebSocket `on message`/`on close` handler's `by` clause does not name the same actor(s) as `on open`'s — the connection's principal is verified once, at `on open`, and carried for its life, so the lifecycle handlers must be the same single actor or the same actor-sum members (binder names may differ).",
    ),
    d(
        "bynk.ws.message_frame_param",
        "A WebSocket `on message` handler does not have exactly one parameter of the service's inbound (`in:`) frame type — the decoded frame (real-time track slice 3b-iii).",
    ),
    d(
        "bynk.ws.open_given_unsupported",
        "A WebSocket `on open` handler declares `given` capabilities — unsupported at v1, since on Workers the handler runs inside the connection-hosting Durable Object, which has no composition root to supply them (real-time track slice 3b).",
    ),
    d(
        "bynk.ws.open_transfer_cross_file",
        "On the Workers target, a WebSocket `on open` handler transfers its `connection` to an agent declared in a different file of the same context — the open is hosted in the agent's Durable Object, which is emitted from the agent's own file, so the upgrade would never be handled; the service and the agent must share a file.",
    ),
    d(
        "bynk.ws.open_transfer_key",
        "On the Workers target, the key of a WebSocket `on open` handler's connection transfer is not a route parameter, a constant of the context or of a commons it uses, or a literal — the upgrade is routed to the hosting Durable Object before the body runs, so the edge must be able to evaluate the key on its own.",
    ),
    d(
        "bynk.ws.open_transfer_shape",
        "A WebSocket `on open` handler does not transfer its `connection` into exactly one agent, so the Workers upgrade has no single Durable Object to route to (real-time track slice 3b).",
    ),
    d(
        "bynk.ws.route_param_mismatch",
        "A WebSocket `on message`/`on close` route parameter does not match the `on open` parameter at the same position — route values are recovered positionally from the connection, so they must be a type-compatible prefix of the `on open` parameters (real-time track slice 3b-iii).",
    ),
];

/// A diagnostic with no single governing grammar construct. `Error` severity.
const fn d(code: &'static str, summary: &'static str) -> DiagnosticInfo {
    DiagnosticInfo {
        code,
        summary,
        grammar_symbol: &[],
        severity: Severity::Error,
    }
}

/// A diagnostic that constrains one or more grammar productions. `Error` severity.
const fn dg(
    code: &'static str,
    summary: &'static str,
    grammar_symbol: &'static [&'static str],
) -> DiagnosticInfo {
    DiagnosticInfo {
        code,
        summary,
        grammar_symbol,
        severity: Severity::Error,
    }
}

/// Downgrades a [`d`]/[`dg`]-built entry to `Warning` severity (ADR 0117) —
/// non-failing, surfaced alongside a clean build. Its call sites here are
/// the single source of truth [`crate::error::Severity::for_error`] reads.
const fn warn(mut info: DiagnosticInfo) -> DiagnosticInfo {
    info.severity = Severity::Warning;
    info
}

/// Downgrades a [`d`]/[`dg`]-built entry to `Hint` severity (#2838) — a
/// non-failing suggestion shown by the editor but not by the terminal.
const fn hint(mut info: DiagnosticInfo) -> DiagnosticInfo {
    info.severity = Severity::Hint;
    info
}

/// The category segment of a code (the part between the first two dots), e.g.
/// `"types"` for `"bynk.types.type_mismatch"`.
pub fn category(code: &str) -> &str {
    code.split('.').nth(1).unwrap_or("")
}

/// A human-readable heading for a category segment.
fn category_title(cat: &str) -> &'static str {
    match cat {
        "agent" | "agents" => "Agents",
        "asset" => "Static assets",
        "boundary" => "Boundaries",
        "capability" => "Capabilities",
        "cloudflare" => "Cloudflare",
        "consumes" => "Consumes",
        "context" => "Contexts",
        "contract" => "Contracts",
        "cron" => "Cron",
        "doc" => "Doc comments",
        "effect" => "Effects",
        "embed" => "Embedded text",
        "expect" => "Expectations",
        "exports" => "Exports",
        "given" => "Given capabilities",
        "http" => "HTTP",
        "lex" => "Lexer",
        "messages" => "Message bundles",
        "mock" => "Mocks (collaborators)",
        "observe" => "Observation",
        "parse" => "Parser",
        "project" => "Project",
        "property" => "Properties (generative tests)",
        "provider" => "Providers",
        "queue" => "Queue",
        "record_spread" => "Record spread",
        "refine" => "Refinement",
        "resolve" => "Resolution",
        "service" => "Services",
        "stability" => "Stability",
        "suite" => "Suites and cases",
        "transition" => "Transitions (step invariants)",
        "types" => "Type checking",
        "uses" => "Uses",
        "val" => "Value fabrication",
        _ => "Other",
    }
}

/// Render the diagnostic index as a Markdown reference page, grouped by
/// category. This is the generator behind
/// `site/src/content/docs/book/reference/diagnostics.md`.
pub fn render_markdown() -> String {
    use std::collections::BTreeMap;

    // Group codes by their category title, preserving sorted code order.
    let mut by_category: BTreeMap<&str, Vec<&DiagnosticInfo>> = BTreeMap::new();
    for info in REGISTRY {
        by_category
            .entry(category_title(category(info.code)))
            .or_default()
            .push(info);
    }

    let mut out = String::new();
    out.push_str("# Diagnostic index\n\n");
    out.push_str(
        "<!-- GENERATED FILE — do not edit by hand.\n     \
         Source: bynk-syntax/src/diagnostics.rs (`render_markdown`).\n     \
         Regenerate with: BYNK_BLESS=1 cargo test -p xtask --test docs_consistency diagnostics_registry:: -->\n\n",
    );
    out.push_str(
        "Every diagnostic code the compiler can emit, with a one-line summary of \
         the cause, grouped by category. For step-by-step cause-and-fix guidance \
         on the most common ones, see the [troubleshooting guides](../troubleshooting/index.md). \
         For a code with a curated explanation, `bynk explain <code>` prints it in a \
         terminal, and in VS Code the diagnostic's **Explain** quick fix (or \
         **Bynk: Explain Diagnostic Code…**) shows the same text in a panel, offline.\n\n",
    );
    out.push_str(&format!(
        "There are **{}** codes in total.\n",
        REGISTRY.len()
    ));

    for (title, infos) in &by_category {
        out.push_str(&format!("\n## {title}\n\n"));
        out.push_str("| Code | Summary | Construct | Severity |\n|---|---|---|---|\n");
        for info in infos {
            // The construct column deep-links each governing production to its
            // entry in the annotated grammar reference; generated from
            // `grammar_symbol` (each value is an embeddable rule, so the
            // `#rule-<raw>` anchor resolves — enforced in diagnostics_registry).
            let construct = info
                .grammar_symbol
                .iter()
                .map(|sym| format!("[`{sym}`](grammar.md#rule-{sym})"))
                .collect::<Vec<_>>()
                .join(", ");
            // A curated (`bynk explain`-able) code links to its Book concept
            // page; the in-site link is validated by the site's link checker,
            // so a moved page or renamed anchor fails the build (#853). Codes
            // without an explanation render as plain inline code.
            let code_cell = match explain(info.code) {
                Some(e) => format!("[`{}`]({})", info.code, e.in_site_link()),
                None => format!("`{}`", info.code),
            };
            // ADR 0117/finding #50: every code's severity, straight from the
            // registry — `Error` (the overwhelming majority) renders as "—"
            // to keep the common case unobtrusive; only a `warn`-built entry
            // shows "Warning".
            let severity = match info.severity {
                Severity::Error => "—",
                Severity::Warning => "Warning",
                Severity::Hint => "Hint",
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                code_cell, info.summary, construct, severity
            ));
        }
    }

    out
}

/// Invert the registry into a `{ "<rule>": [ { code, summary }, … ], … }` map,
/// serialised as pretty JSON with sorted keys and sorted codes. Only rules with
/// at least one diagnostic appear. This is the generator behind
/// `docs/grammar-semantics.json`, which the `{{#grammar-semantics <rule>}}`
/// preprocessor directive consumes.
pub fn render_grammar_semantics_json() -> String {
    use std::collections::BTreeMap;

    // REGISTRY is sorted by code, so each rule's vector comes out code-sorted;
    // the BTreeMap gives sorted rule names.
    let mut by_symbol: BTreeMap<&str, Vec<&DiagnosticInfo>> = BTreeMap::new();
    for info in REGISTRY {
        for sym in info.grammar_symbol {
            by_symbol.entry(sym).or_default().push(info);
        }
    }

    let mut map = serde_json::Map::new();
    map.insert(
        "_generated".to_string(),
        serde_json::Value::String(
            "Generated from the grammar_symbol field of bynk-syntax/src/diagnostics.rs. \
             Do not edit by hand. Regenerate with: BYNK_BLESS=1 cargo test -p \
             xtask --test docs_consistency diagnostics_registry::"
                .to_string(),
        ),
    );
    for (sym, infos) in by_symbol {
        let arr: Vec<serde_json::Value> = infos
            .iter()
            .map(|info| serde_json::json!({ "code": info.code, "summary": info.summary }))
            .collect();
        map.insert(sym.to_string(), serde_json::Value::Array(arr));
    }

    let mut s =
        serde_json::to_string_pretty(&serde_json::Value::Object(map)).expect("serialise semantics");
    s.push('\n');
    s
}

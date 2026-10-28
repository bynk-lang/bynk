//! #1969: the cross-context wire format, version 1, as a published surface.
//!
//! The interop track's D1 (`design/archive/retired-tracks.md`, interop track) makes the format a
//! caller outside the project can speak a contract with its own version,
//! apart from the compiler's: the path, method and headers of a call; the
//! order of the callee's checks, with their status codes and error bodies; the
//! JSON encoding of each boundary type; and the contract hash.
//!
//! This module is the single source for the parts that are constants. The
//! emitter builds the `/_bynk/call/` dispatch from [`CallCheck`] and the header
//! names below, and [`mod@reference`] renders `reference/wire-format.md` from the
//! same values plus [`crate::wire`]'s encoding model and [`crate::contract`]'s
//! hash. `bynkc/tests/wire_reference.rs` holds the page, the emitted dispatch,
//! the runtime's `callService` and the generated codecs to it.
//!
//! **Stability (version 1).** Additive only: a new status code, a new error
//! `kind` or a new optional header may be added within the version. Changing an
//! existing encoding, check, status or the hash's normal form is a new wire
//! version, which a callee accepts beside the old one for an overlap window
//! ([`ACCEPTED_WIRE_VERSIONS`]). That includes the request signature
//! ([`signing_input`]): its canonical input and its headers are part of the
//! version, the keys are not.

pub mod reference;

/// The wire version this compiler's callers speak.
pub const WIRE_VERSION: u32 = 1;

/// The wire versions this compiler's callees accept. A request without
/// [`WIRE_HEADER`] is version 1.
pub const ACCEPTED_WIRE_VERSIONS: &[u32] = &[1];

/// The path prefix a call is addressed to; the service name follows it.
pub const CALL_PATH_PREFIX: &str = "/_bynk/call/";

/// The only method a call may use.
pub const CALL_METHOD: &str = "POST";

/// The caller's compiled hash of the callee's contract.
pub const CONTRACT_HEADER: &str = "X-Bynk-Contract";

/// The calling context's qualified name.
pub const CALLER_HEADER: &str = "X-Bynk-Caller";

/// The wire version the caller speaks; absent means 1.
pub const WIRE_HEADER: &str = "X-Bynk-Wire";

/// I2 (#1971): when the caller signed the request, in whole seconds since the
/// Unix epoch, as a decimal integer.
pub const TIMESTAMP_HEADER: &str = "X-Bynk-Timestamp";

/// I2 (#1971): the request's signature, HMAC-SHA256 over [`signing_input`]
/// under the edge's key, as 64 lowercase hex digits.
pub const SIGNATURE_HEADER: &str = "X-Bynk-Signature";

/// I2 (#1971): how far, in seconds, a request's timestamp may be from the
/// callee's clock, either way. A requirement, not a tuning knob (ADR
/// `per-edge-call-signature`): it bounds how long a captured request can be
/// replayed, and nothing else does.
pub const SIGNATURE_WINDOW_SECONDS: u64 = 300;

/// I2 (#1971): the largest body a callee reads, in bytes. The body is read
/// under this cap before it is hashed, so a sender without a key cannot make
/// the callee buffer and hash an unbounded one.
pub const MAX_BODY_BYTES: usize = 1_048_576;

/// I2 (#1971): the first line of [`signing_input`], naming the scheme, so a
/// signature over this input can never verify as one over another format's.
pub const SIGNING_SCHEME: &str = "BYNK-CALL-1";

/// I2 (#1971): the secret a callee holds for the edge from one caller,
/// [`edge_key_from`].
pub const EDGE_FROM_PREFIX: &str = "BYNK_CALL_FROM_";

/// I2 (#1971): the secret a caller holds for the edge to one callee,
/// [`edge_key_to`].
pub const EDGE_TO_PREFIX: &str = "BYNK_CALL_TO_";

/// State-migrations S4 (#2259 Decision D): the prefix of `bynk migrate
/// sweep`'s wake route, `/_bynk/migrate/<Class>`. Not part of the published
/// call wire format: only the toolchain calls it.
pub const MIGRATE_PATH_PREFIX: &str = "/_bynk/migrate/";

/// State-migrations S4 (#2259): the caller name a sweep signs as. The route
/// admits no other caller, so a context's edge key cannot wake instances.
pub const MIGRATE_CALLER: &str = "bynk.migrate";

/// State-migrations S4 (#2259): the secret a sweep's key is set under for the
/// sweep's duration. Without it the route answers `404`.
pub const MIGRATE_KEY: &str = "BYNK_MIGRATE_KEY";

/// I2 (#1971): whether `context` is a name an edge key can be derived from:
/// dot-separated segments, each a lowercase letter, then lowercase letters
/// and digits, with single underscores between runs of them (`shop.orders`,
/// `a_b.c2`). On that domain [`edge_suffix`] is injective, which is what
/// keeps one caller's key from verifying another's name.
pub fn is_edge_name(context: &str) -> bool {
    !context.is_empty()
        && context.split('.').all(|segment| {
            let bytes = segment.as_bytes();
            bytes.first().is_some_and(u8::is_ascii_lowercase)
                && bytes.last() != Some(&b'_')
                && !segment.contains("__")
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
        })
}

/// I2 (#1971): the part of an edge key's name that names the context at its
/// other end: upper case, a dot written `__` (`shop.orders` → `SHOP__ORDERS`).
/// `None` for a name outside [`is_edge_name`]'s domain.
pub fn edge_suffix(context: &str) -> Option<String> {
    is_edge_name(context).then(|| context.to_ascii_uppercase().replace('.', "__"))
}

/// I2 (#1971): the secret the **callee** holds for calls from `caller`
/// (`BYNK_CALL_FROM_SHOP__ORDERS`). The callee picks it by the request's
/// [`CALLER_HEADER`], so an undeclared caller finds no key and is refused.
pub fn edge_key_from(caller: &str) -> Option<String> {
    edge_suffix(caller).map(|s| format!("{EDGE_FROM_PREFIX}{s}"))
}

/// I2 (#1971): the secret the **caller** holds for calls to `callee`
/// (`BYNK_CALL_TO_SHOP__PAYMENT`). Same value as the callee's
/// [`edge_key_from`] for the same edge.
pub fn edge_key_to(callee: &str) -> Option<String> {
    edge_suffix(callee).map(|s| format!("{EDGE_TO_PREFIX}{s}"))
}

/// I2 (#1971): the bytes a request's signature is computed over. One field
/// per line, `\n`-separated, no trailing newline:
///
/// 1. [`SIGNING_SCHEME`];
/// 2. the caller, as sent in [`CALLER_HEADER`];
/// 3. the callee's qualified context name, so a signature verifies at one
///    callee only;
/// 4. the method;
/// 5. the URL's pathname exactly as sent, with no query string and no
///    percent-normalisation;
/// 6. [`CONTRACT_HEADER`] as sent, empty when absent;
/// 7. [`WIRE_HEADER`] as sent, `1` when absent, so the version a callee
///    decodes under is the one that was signed;
/// 8. [`TIMESTAMP_HEADER`] as sent;
/// 9. the SHA-256 of the body's bytes, 64 lowercase hex digits.
///
/// Every header the callee acts on is in it. Header values cannot hold a
/// newline, so no field can forge another's line.
#[allow(clippy::too_many_arguments)]
pub fn signing_input(
    caller: &str,
    callee: &str,
    method: &str,
    path: &str,
    contract: &str,
    wire: &str,
    timestamp: &str,
    body_sha256_hex: &str,
) -> String {
    [
        SIGNING_SCHEME,
        caller,
        callee,
        method,
        path,
        contract,
        wire,
        timestamp,
        body_sha256_hex,
    ]
    .join("\n")
}

/// The `details` of a `MalformedJson` refusal. The HTTP routes use the same.
pub const MALFORMED_JSON_DETAILS: &str = "Invalid request body";

/// One header of a call request, as the reference documents it.
pub struct Header {
    pub name: &'static str,
    pub required: &'static str,
    pub meaning: &'static str,
}

/// Every header a call carries, in the order the reference lists them.
pub const HEADERS: &[Header] = &[
    Header {
        name: CONTRACT_HEADER,
        required: "yes",
        meaning: "The caller's hash of the service's contract, 16 lowercase hex digits. Must equal the callee's, or one the callee declares it still accepts ([Contract skew](/book/guides/projects-build-and-deployment/contract-skew/#deploying-a-callee-ahead-of-its-callers)); a missing header does not.",
    },
    Header {
        name: CALLER_HEADER,
        required: "yes",
        meaning: "The calling context's qualified name (`shop.orders`), which the handler reads as `c.identity`, or a foreign caller's actor name (`Legacy`). Selects the key the signature is checked under, so it is proved, not claimed.",
    },
    Header {
        name: TIMESTAMP_HEADER,
        required: "yes",
        meaning: "When the caller signed the request, in whole seconds since the Unix epoch.",
    },
    Header {
        name: SIGNATURE_HEADER,
        required: "yes",
        meaning: "HMAC-SHA256 of the signing input under the edge's key, 64 lowercase hex digits. See *Authentication* below.",
    },
    Header {
        name: WIRE_HEADER,
        required: "no",
        meaning: "The wire version the caller speaks, exactly the decimal digits of an accepted version (`1`, not `01` or `+1`). Absent means `1`.",
    },
    Header {
        name: "content-type",
        required: "no",
        meaning: "Bynk callers send `application/json`. The callee does not check it.",
    },
];

/// One check the callee makes on a call, in the published order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallCheck {
    Method,
    BodyTooLarge,
    Authentication,
    WireVersion,
    UnknownService,
    NotAdmitted,
    Contract,
    MalformedJson,
    Arguments,
}

/// The callee's checks, in the order it makes them. The first that fails
/// answers the request.
///
/// I2 (#1971): a signature is checked under the key the caller header
/// selects, so a call without a caller fails [`CallCheck::Authentication`];
/// there is no separate missing-caller check after it.
pub const CALL_CHECKS: [CallCheck; 9] = [
    CallCheck::Method,
    CallCheck::BodyTooLarge,
    CallCheck::Authentication,
    CallCheck::WireVersion,
    CallCheck::UnknownService,
    CallCheck::NotAdmitted,
    CallCheck::Contract,
    CallCheck::MalformedJson,
    CallCheck::Arguments,
];

impl CallCheck {
    /// The status a failure of this check answers with.
    pub fn status(self) -> u16 {
        match self {
            CallCheck::Method => 405,
            CallCheck::BodyTooLarge => 413,
            CallCheck::Authentication => 401,
            CallCheck::WireVersion => 400,
            CallCheck::UnknownService => 404,
            CallCheck::NotAdmitted => 403,
            CallCheck::Contract => 409,
            CallCheck::MalformedJson => 400,
            CallCheck::Arguments => 400,
        }
    }

    /// The `kind` of the JSON error body, for a check that answers with one.
    /// `Arguments` answers with the codec's own `StructuralMismatch` or
    /// `RefinementViolation`, so it has no single kind.
    pub fn kind(self) -> Option<&'static str> {
        match self {
            CallCheck::WireVersion => Some("UnsupportedWireVersion"),
            CallCheck::Contract => Some("ContractMismatch"),
            CallCheck::MalformedJson => Some("MalformedJson"),
            CallCheck::Method
            | CallCheck::BodyTooLarge
            | CallCheck::Authentication
            | CallCheck::UnknownService
            | CallCheck::NotAdmitted
            | CallCheck::Arguments => None,
        }
    }

    /// The plain-text body of a check that answers with one.
    pub fn text_body(self) -> Option<&'static str> {
        match self {
            CallCheck::BodyTooLarge => Some("Payload Too Large"),
            CallCheck::Authentication => Some("Unauthorized"),
            CallCheck::UnknownService => Some("Not found"),
            CallCheck::NotAdmitted => Some("Forbidden"),
            _ => None,
        }
    }

    /// What the check tests, for the reference.
    pub fn condition(self) -> &'static str {
        match self {
            CallCheck::Method => "The method is `POST`.",
            CallCheck::BodyTooLarge => {
                "The body is at most 1 MiB (1048576 bytes). Read under the cap, before anything is hashed."
            }
            CallCheck::Authentication => {
                "`X-Bynk-Signature` verifies, under the key the callee holds for the edge from `X-Bynk-Caller`, over the signing input, and `X-Bynk-Timestamp` is within 300 seconds of the callee's clock. Compared in constant time."
            }
            CallCheck::WireVersion => {
                "`X-Bynk-Wire` is absent or names a version the callee accepts."
            }
            CallCheck::UnknownService => {
                "The path names a service of the callee's context with an `on call` handler."
            }
            CallCheck::NotAdmitted => {
                "The service's handler admits the caller the signature proved: a Bynk context by `Caller` (the default), a foreign caller only by its own `Foreign` actor."
            }
            CallCheck::Contract => "`X-Bynk-Contract` equals the callee's hash of that service, or one of the older hashes the callee accepts for it.",
            CallCheck::MalformedJson => "The body parses as JSON.",
            CallCheck::Arguments => {
                "The body decodes as the handler's parameters (see *Encoding*)."
            }
        }
    }

    /// The response a failure gives, for the reference.
    pub fn response(self) -> String {
        let text = match self {
            CallCheck::Method => "empty body, `Allow: POST`",
            CallCheck::BodyTooLarge => "text `Payload Too Large`",
            CallCheck::Authentication => "text `Unauthorized`",
            CallCheck::WireVersion => {
                let accepted: Vec<String> =
                    ACCEPTED_WIRE_VERSIONS.iter().map(u32::to_string).collect();
                return format!(
                    "`{{\"kind\":\"UnsupportedWireVersion\",\"accepted\":[{}]}}`",
                    accepted.join(",")
                );
            }
            CallCheck::UnknownService => "text `Not found`",
            CallCheck::NotAdmitted => "text `Forbidden`",
            CallCheck::Contract => {
                r#"`{"kind":"ContractMismatch","service":"…","expected":"…","actual":"…"}`"#
            }
            CallCheck::MalformedJson => {
                r#"`{"kind":"MalformedJson","details":"Invalid request body"}`"#
            }
            CallCheck::Arguments => "a `StructuralMismatch` or `RefinementViolation` body",
        };
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_keys_name_the_context_at_the_other_end() {
        assert_eq!(
            edge_key_from("shop.orders").as_deref(),
            Some("BYNK_CALL_FROM_SHOP__ORDERS")
        );
        assert_eq!(
            edge_key_to("shop.payment").as_deref(),
            Some("BYNK_CALL_TO_SHOP__PAYMENT")
        );
        assert_eq!(edge_suffix("a_b.c2").as_deref(), Some("A_B__C2"));
    }

    #[test]
    fn edge_suffixes_never_collide_on_their_domain() {
        // A dot and an underscore stay apart, which a plain
        // `replace('.', "_")` would merge.
        assert_ne!(edge_suffix("a.b"), edge_suffix("a_b"));
        // Names that would make the encoding ambiguous are outside it.
        for bad in [
            "", "a..b", ".a", "a.", "a__b", "_a", "a_", "a._b", "A.b", "a-b", "1a", "a b",
        ] {
            assert!(!is_edge_name(bad), "{bad:?} is not an edge name");
            assert_eq!(edge_key_from(bad), None);
        }
        for good in ["integration", "shop.orders", "a_b.c2", "x1.y_2_z"] {
            assert!(is_edge_name(good), "{good:?} is an edge name");
        }
    }

    /// I3 (#1971): the invariant that keeps foreign callers and contexts apart
    /// on the wire. A foreign actor's name starts with a capital letter
    /// (`bynk.actor.foreign_name_is_context_name`), and no name that does is
    /// an edge name, so no foreign caller's name selects an edge key. The
    /// runtime's `edgeSuffix` pins the same property.
    #[test]
    fn no_capitalised_name_is_an_edge_name() {
        for name in ["Legacy", "L", "Legacy.orders", "LEGACY", "Ledger_2"] {
            assert!(!is_edge_name(name), "{name:?}");
            assert_eq!(edge_key_from(name), None, "{name:?}");
        }
    }

    #[test]
    fn the_signing_input_is_one_field_per_line() {
        assert_eq!(
            signing_input(
                "shop.orders",
                "shop.payment",
                "POST",
                "/_bynk/call/authorise",
                "0123456789abcdef",
                "1",
                "1700000000",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            "BYNK-CALL-1\nshop.orders\nshop.payment\nPOST\n/_bynk/call/authorise\n0123456789abcdef\n1\n1700000000\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_checks_state_the_published_limits() {
        assert!(
            CallCheck::BodyTooLarge
                .condition()
                .contains(&MAX_BODY_BYTES.to_string())
        );
        assert!(
            CallCheck::Authentication
                .condition()
                .contains(&format!("{SIGNATURE_WINDOW_SECONDS} seconds"))
        );
    }
}

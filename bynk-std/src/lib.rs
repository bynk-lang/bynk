//! The Bynk standard library's first-party sources, as data.
//!
//! The `bynk` conformance surface, the Bynk-written commons (`bynk.list`,
//! `bynk.map`, `bynk.string`, `bynk.locale`, `bynk.locale.types`) and the
//! `bynk.cloudflare` platform adapter ship with the compiler rather than being
//! authored by the user. This crate holds their `.bynk` sources, their
//! per-platform TypeScript bindings and the metadata the toolchain reads about
//! them, embedded with `include_str!`. It holds no logic: `bynk-check` parses and
//! injects these sources when a project `uses` or `consumes` them, and provides
//! each binding for the selected [`Platform`]. The `bynk` root namespace is
//! reserved (`bynk.namespace.reserved`) so user code can never collide with it.
//!
//! The sources live under `lib/`: the portable layers in `lib/core/` (with the
//! `bynk` surface's bindings in `lib/core/bindings/`) and the platform adapter in
//! `lib/cloudflare/`. Nothing in `lib/core/` depends on `lib/cloudflare/`, so the
//! platform layer can be split out later without renaming a unit.

/// The deploy platform — a selection axis distinct from the `--target
/// {bundle,workers}` emit mode (§6.2). It chooses which `bynk-<platform>.ts`
/// binding is linked for the `bynk` surface. v0.17 shipped `cloudflare`;
/// v0.18 adds `node`, making the axis observable (and giving v0.19's
/// platform-lock enforcement a second platform to fire against).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Platform {
    #[default]
    Cloudflare,
    Node,
    /// The browser (in-browser track, slice 1 — the REPL/playground host). A
    /// Tier-3 binding exposing the `bynk` surface over Web APIs, composed with
    /// `BuildTarget::Bundle` only (a browser cannot do the Workers wire model).
    Browser,
}

impl Platform {
    /// The output filename of this platform's `bynk` binding module.
    pub fn bynk_binding_filename(self) -> &'static str {
        match self {
            Platform::Cloudflare => "bynk-cloudflare.ts",
            Platform::Node => "bynk-node.ts",
            Platform::Browser => "bynk-browser.ts",
        }
    }

    /// The TypeScript source of this platform's `bynk` binding.
    pub fn bynk_binding_source(self) -> &'static str {
        match self {
            Platform::Cloudflare => BYNK_CLOUDFLARE_BINDING,
            Platform::Node => BYNK_NODE_BINDING,
            Platform::Browser => BYNK_BROWSER_BINDING,
        }
    }

    /// The platform's stable name (for the `--platform` flag and diagnostics).
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Cloudflare => "cloudflare",
            Platform::Node => "node",
            Platform::Browser => "browser",
        }
    }
}

/// The unit name of the reserved first-party surface adapter.
pub const BYNK_UNIT: &str = "bynk";

/// The capabilities the env-free `bynk` surface exports (its `exports
/// capability { … }` line in `bynk.bynk`). First-party symbols live on
/// synthetic files and are excluded from the binding index, so the
/// unknown-name quick-fix (#852) offering `consumes bynk { … }` needs this
/// explicit list. Kept in sync with the adapter source by
/// `bynk_surface_capabilities_match_the_adapter`.
pub const BYNK_SURFACE_CAPABILITIES: &[&str] = &[
    "Clock",
    "Random",
    "Logger",
    "Fetch",
    "Secrets",
    "Locale",
    "Idempotency",
    "Events",
];

/// The unit name of the first-party Cloudflare platform adapter (v0.19,
/// decision 0026): inside the reserved `bynk.*` prefix, so no separate
/// reservation rule is needed. The surface unit `bynk` stays the portability
/// marker; `bynk.<platform>` units are the platform-locked ones.
pub const CLOUDFLARE_UNIT: &str = "bynk.cloudflare";

/// The fixed Worker binding name for the Kv namespace (decision C1, v0.19):
/// one namespace, one `[[kv_namespaces]]` stanza, one `env.KV` field.
pub const KV_BINDING_NAME: &str = "KV";

/// v0.18 decision 0021 / v0.19 decision 0025: which first-party provider
/// classes take the Worker `env` as a constructor argument, keyed by
/// (unit, provider class). `SecretsProvider` accepts an optional env with a
/// `globalThis` probe fallback; `WorkersKv` *requires* env on use — KV
/// namespaces exist only on the Worker `env`, never on `globalThis`.
///
/// A view over [`UNITS`]: each entry's [`Unit::env_providers`].
pub fn provider_takes_env(unit: &str, provider: &str) -> bool {
    self::unit(unit).is_some_and(|u| u.env_providers.contains(&provider))
}

/// v0.19 decision 0024: which first-party units are **platform-native** —
/// consuming one locks its deployment unit to the returned platform. This is
/// the metadata that drives effective-platform computation, `env` resource
/// typing, and `wrangler.toml` stanza derivation; no user-facing marker
/// syntax exists (additive later, when third-party platform adapters become
/// a goal).
///
/// A view over [`UNITS`]: each entry's [`Unit::platform`].
pub fn platform_of(unit: &str) -> Option<Platform> {
    self::unit(unit).and_then(|u| u.platform)
}

/// The unit names of the first-party collection commons (v0.20b): the
/// Bynk-written combinator stdlib over the built-in `List`/`Map` kernel.
/// Inside the reserved `bynk.*` prefix; injected when `uses`-imported.
pub const LIST_UNIT: &str = "bynk.list";
pub const MAP_UNIT: &str = "bynk.map";

/// `bynk.list` — combinators over the `List` kernel (`fold`, `prepend`,
/// `length`, `get`, `foldEff`), written in ordinary Bynk (decision 0034):
/// the first real consumer of v0.20a generics, lambdas, and effectful
/// traversal. Order-preserving combinators build with `fold` + `prepend`
/// and a final `reverse` — O(n) builds, never `append` (which would be
/// O(n²) over the array lowering).
pub const BYNK_LIST_SRC: &str = include_str!("../lib/core/bynk.list.bynk");

/// `bynk.map` — combinators over the `Map` kernel (`empty`, `insert`, `get`,
/// `keys`, `length`). `fromList` is deliberately absent: Bynk has no pair
/// type to spell a `List[(K, V)]` with, so map construction is `Map.empty()`
/// + `insert` (revisit with tuples or generic records).
pub const BYNK_MAP_SRC: &str = include_str!("../lib/core/bynk.map.bynk");

/// Inside the reserved `bynk.*` prefix; injected when `uses`-imported.
pub const STRING_UNIT: &str = "bynk.string";

/// `bynk.string` — Bynk-written helpers over the v0.22a string kernel
/// (`concat`, the `List` `fold`, and the `Option` kernel methods). The
/// kernel itself is compiler built-in (ADR 0046); only derived helpers
/// live here. `join` folds to `Option[String]` so empty-string *elements*
/// are joined faithfully (a bare `""` accumulator could not tell "nothing
/// yet" from "first element was empty").
pub const BYNK_STRING_SRC: &str = include_str!("../lib/core/bynk.string.bynk");

/// Inside the reserved `bynk.*` prefix; injected when `uses`-imported.
pub const URL_UNIT: &str = "bynk.url";

/// `bynk.url` (standard-library K2, #2594) — a `Url` record with RFC 3986
/// `parse`/`format` and query-string helpers, written in Bynk over the
/// `String` kernel. Only percent-encoding is kernel (`percentEncode`,
/// `percentDecode`): it needs a string's UTF-8 bytes. `Url` is a record here,
/// not a base type, because a base type's name is a lexer token, and `Url`
/// as a keyword would break every user `type Url`.
pub const BYNK_URL_SRC: &str = include_str!("../lib/core/bynk.url.bynk");

/// Inside the reserved `bynk.*` prefix; injected when `uses`-imported by
/// application code (a message-bundle commons, ADR 0272) — never directly by
/// the `bynk` adapter itself any more (see [`LOCALE_TYPES_UNIT`]).
pub const LOCALE_UNIT: &str = "bynk.locale";

/// `bynk.locale` — the bundle-free `render`/`renderArg` helpers and the
/// `message`/`withText`/`withWhole`/`withNum`/`withMoment` builder API
/// (slice 1: `render` formats `code` plus a deterministic sorted-by-key
/// rendering of `params`; no lookup, no locale-dependent behaviour). `uses
/// bynk.locale.types` for the types its own signatures name.
pub const BYNK_LOCALE_SRC: &str = include_str!("../lib/core/bynk.locale.bynk");

/// Inside the reserved `bynk.*` prefix; injected when `uses`-imported. Also
/// `uses`-imported by the `bynk` adapter itself, to give `capability Locale`'s
/// `current() -> Effect[LocaleTag]` a type to name (an adapter may `uses` a
/// commons; a commons cannot in turn reference an adapter). Split out of
/// `bynk.locale` (locale-negotiation-slice-2 follow-up, #886) specifically so
/// a context can reach `LocaleTag` without also pulling in `bynk.locale`'s
/// own `render` — which previously collided with a message-bundle commons's
/// own synthesised `render` (ADR 0272) the moment a context needed both.
pub const LOCALE_TYPES_UNIT: &str = "bynk.locale.types";

/// `bynk.locale.types` — `LocaleTag`, `MessageArg`, `Message`: a
/// dependency-free leaf (no `uses` of its own) so any consumer, including
/// the `bynk` adapter itself, can reach these types without also reaching
/// `bynk.locale`'s value-level API.
pub const BYNK_LOCALE_TYPES_SRC: &str = include_str!("../lib/core/bynk.locale.types.bynk");

/// The reserved `bynk` conformance-surface adapter (env-free core). It has no
/// `binding` clause — the toolchain supplies one per platform (see
/// [`Platform::bynk_binding_source`]).
pub const BYNK_ADAPTER_SRC: &str = include_str!("../lib/core/bynk.bynk");

/// The Cloudflare binding for the `bynk` surface. Implements the canonical
/// provider symbols against the platform host API. The refined `Uuid` is built
/// through its emitted validating `.of` constructor (§4.4), treating the
/// unreachable `Err` as a bug rather than trusting the value away.
const BYNK_CLOUDFLARE_BINDING: &str = include_str!("../lib/core/bindings/bynk-cloudflare.ts");

/// The Node (≥ the toolchain's `NODE_MAJOR_FLOOR`, in `bynk-emit`) binding for
/// the `bynk` surface (v0.18). Deliberately
/// near-identical to the cloudflare binding: `Date.now`, the global
/// `crypto`/`fetch`, and `console` are the same host API on both runtimes —
/// which is exactly the ambient-surface portability claim (spec §4.2). The
/// `SecretsProvider` reads `process.env` through the same `globalThis` probe
/// (never bare `process`, which would demand @types/node at the tsc gate).
const BYNK_NODE_BINDING: &str = include_str!("../lib/core/bindings/bynk-node.ts");

/// The Browser binding for the `bynk` surface (in-browser track, slice 1). Tier-3:
/// the same capability surface over Web APIs. `Clock`/`Random`/`Logger` are
/// byte-identical to the Node binding (`Date.now`, Web Crypto `crypto.randomUUID`,
/// `console` — all Web standards); the two substitutions are the playground safety
/// boundary — `Fetch` is **withheld** (throws; no outbound egress from the
/// playground origin) and `Secrets` is **unavailable** (throws; a browser has no
/// secret store), both failing loudly rather than silently degrading.
const BYNK_BROWSER_BINDING: &str = include_str!("../lib/core/bindings/bynk-browser.ts");

/// The first-party Cloudflare platform adapter (v0.19): the platform's real
/// infrastructure capabilities, as they are — no portable intersection
/// (decision 0016). The v0.19 surface was the minimal, collection-free `Kv`
/// (decision 0023); v0.23 adds the `list` drain and `putTtl` (0050/0051);
/// structured values are v0.22-codec composition, and `Queue` remains its
/// own future increment. Like the `bynk` surface it has no `binding`
/// clause — the toolchain supplies the binding.
pub const CLOUDFLARE_ADAPTER_SRC: &str = include_str!("../lib/cloudflare/bynk.cloudflare.bynk");

/// Every first-party `.bynk` source, paired with its unit name — the single
/// list every consumer iterates: hover (`describe_firstparty_symbol`),
/// completion (`EMBEDDED_UNITS`), and the parse guard
/// (`every_first_party_source_parses`). Before this list each consumer kept its
/// own hand-maintained copy, and `bynk.locale`/`bynk.locale.types` were added to
/// none of them — so the whole locale surface hovered as nothing and was absent
/// from completion (#901).
///
/// A view over [`UNITS`], in its order, which is observable: hover takes the
/// first source declaring a name, and completion lists units in this order. The
/// `firstparty_sources_cover_every_src_const` drift guard
/// (`bynkc/tests/firstparty_sources.rs`) fails if a `*_SRC` const is left out of
/// [`UNITS`].
pub const FIRSTPARTY_SOURCES: &[(&str, &str)] = &SOURCES;

/// [`FIRSTPARTY_SOURCES`]' backing array, built from [`UNITS`] at compile time.
const SOURCES: [(&str, &str); UNITS.len()] = {
    let mut out = [("", ""); UNITS.len()];
    let mut i = 0;
    while i < UNITS.len() {
        out[i] = (UNITS[i].name, UNITS[i].source);
        i += 1;
    }
    out
};

/// The output path of the Cloudflare platform adapter's binding module,
/// beside the adapter's emitted `bynk/cloudflare.ts` (distinct from the
/// `bynk` *surface*'s per-platform `bynk-cloudflare.ts`).
pub const CLOUDFLARE_BINDING_FILENAME: &str = "bynk/cloudflare.binding.ts";

/// The Cloudflare platform adapter's binding. `WorkersKv` reads the Worker
/// `env` explicitly (decision 0025): KV namespaces exist only on `env` —
/// there is no `globalThis` path — so a missing binding is a clear runtime
/// error rather than a silent fallback.
const CLOUDFLARE_BINDING: &str = include_str!("../lib/cloudflare/cloudflare.binding.ts");

/// The toolchain-supplied binding for the Cloudflare platform adapter.
pub fn cloudflare_binding_source() -> &'static str {
    CLOUDFLARE_BINDING
}

/// The unit name of the first-party cryptography adapter (standard-library K4,
/// D5/D6): a flat sibling of `bynk`, platform-free, with a binding per platform.
pub const CRYPTO_UNIT: &str = "bynk.crypto";

/// `bynk.crypto` — digests, HMAC, constant-time comparison, and signing and
/// verification with imported keys, as `Effect`s over Web Crypto.
pub const BYNK_CRYPTO_SRC: &str = include_str!("../lib/crypto/bynk.crypto.bynk");

/// The Cloudflare Workers binding for `bynk.crypto`. The three differ only where
/// the platforms do: the constant-time comparison primitive.
const CRYPTO_CLOUDFLARE_BINDING: &str = include_str!("../lib/crypto/crypto-cloudflare.binding.ts");

/// The Node binding for `bynk.crypto`.
const CRYPTO_NODE_BINDING: &str = include_str!("../lib/crypto/crypto-node.binding.ts");

/// The browser binding for `bynk.crypto`. Web Crypto exists only in a secure
/// context (R14.5): outside one it throws, loudly, rather than degrade.
const CRYPTO_BROWSER_BINDING: &str = include_str!("../lib/crypto/crypto-browser.binding.ts");

/// The opaque key types `bynk.crypto` exports. A value of one holds a
/// non-extractable platform key, so it is never stored, persisted, logged or
/// sent (`bynk.types.key_at_boundary`), as a `Stream` is not. The checker finds
/// them by the unit's reserved span file, so a user type of the same name is not
/// mistaken for one.
pub const CRYPTO_KEY_TYPES: &[&str] = &["SigningKey", "VerifyKey"];

/// What a first-party unit is, and so how a project brings it in: an adapter is
/// injected when a unit `consumes` it, a commons when a unit `uses` it. Local to
/// this crate, which has no dependencies; `bynk-check` maps it onto its own
/// unit kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Adapter,
    Commons,
}

/// The TypeScript binding the toolchain supplies for a first-party unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// A commons: Bynk-written, so nothing to bind.
    None,
    /// One binding per [`Platform`] ([`Platform::bynk_binding_filename`] and
    /// [`Platform::bynk_binding_source`]): the portable `bynk` surface.
    PerPlatform,
    /// A single binding, for a platform-native adapter that runs only on its own
    /// platform (the platform lock rejects every other `--platform`).
    Fixed {
        filename: &'static str,
        source: &'static str,
    },
    /// One binding file per [`Platform`], each with its own name and source, for a
    /// platform-free unit whose binding differs where the platforms do
    /// (`bynk.crypto`). [`Binding::PerPlatform`] stays the `bynk` surface's.
    PerPlatformFiles {
        cloudflare: (&'static str, &'static str),
        node: (&'static str, &'static str),
        browser: (&'static str, &'static str),
    },
}

/// One first-party unit, as data (standard-library track, S1 #1945). Adding a
/// unit to the standard library is an entry in [`UNITS`] plus its files; the
/// checker's injection reads this table and needs no change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unit {
    /// The unit's dotted name, inside the reserved `bynk.*` prefix.
    pub name: &'static str,
    /// Whether a project `consumes` it (an adapter) or `uses` it (a commons).
    pub kind: UnitKind,
    /// The synthetic path the injected unit is parsed under. Diagnostics and
    /// emission name the unit by it, so it never changes.
    pub identity_path: &'static str,
    /// The unit's `.bynk` source.
    pub source: &'static str,
    /// The binding the toolchain supplies for it.
    pub binding: Binding,
    /// Its permanent `ExprId` block: the checker parses it at
    /// `FIRSTPARTY_ID_BASE + id_block * FIRSTPARTY_ID_BLOCK`, and injects units
    /// in ascending `id_block` order. A block is never reused or moved, so ids
    /// and injection order stay stable as units are added; a new unit takes the
    /// next free block.
    pub id_block: u32,
    /// The platform that consuming it locks a deployment unit to (decision
    /// 0024), if it is platform-native.
    pub platform: Option<Platform>,
    /// The provider classes whose constructor takes the Worker `env` (decisions
    /// 0021/0025).
    pub env_providers: &'static [&'static str],
    /// The unit's stability (track D2, S3 #1963): a stable item is under the
    /// 1.0 promise, an experimental one carries none and needs an explicit
    /// `[unstable] allow` to use. Required, so no entry is added without a
    /// choice.
    pub stability: Stability,
    /// Items inside a stable unit that are experimental, as paths relative to
    /// the unit: an exported item (`"Kv"`) or a capability operation
    /// (`"Kv.list"`). An experimental item makes its operations experimental.
    pub experimental: &'static [&'static str],
    /// The exported enums that may gain variants in a minor release, so a
    /// `match` covering one needs a wildcard (track §11 Q5).
    pub extensible: &'static [&'static str],
    /// The capabilities a context may declare named instances of with
    /// `capability Sessions = Kv` (S4a, #1966): each its own platform resource.
    /// A capability not listed (Workers AI's single `[ai]` binding, say) has no
    /// instances.
    pub instantiable: &'static [Instantiable],
}

/// A capability a context may declare instances of (S4a, #1966, track D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instantiable {
    /// The capability's name in its unit (`Kv`).
    pub capability: &'static str,
    /// The prefix of each instance's Worker binding name: `KV` makes
    /// `capability Sessions = Kv` bind as `KV_SESSIONS`.
    pub binding_prefix: &'static str,
    /// The creation options (`@name(…)`) an instance may carry, each with the
    /// one literal its value is (standard-library P3, #2342: `@worker("…")`
    /// takes a string, `@limit(100)` an `Int`, `@period(60.seconds)` a
    /// `Duration`); any other is refused. Empty when the kind takes none yet.
    pub options: &'static [(&'static str, OptionKind)],
    /// The options every instance must carry (interop I5, #1972): a
    /// `Service` names the Worker it reaches, so it has no default. A subset
    /// of [`Instantiable::options`].
    pub required: &'static [&'static str],
    /// Whether the base capability is usable bare, through `given Kv`, as
    /// well as through instances. `false` for a kind whose every use needs an
    /// option: a bare `Service` would name no Worker.
    pub bare: bool,
}

impl Instantiable {
    /// The kind of literal option `name` takes, or `None` when it is not one
    /// of this kind's options.
    pub fn option_kind(&self, name: &str) -> Option<OptionKind> {
        self.options
            .iter()
            .find(|(o, _)| *o == name)
            .map(|(_, k)| *k)
    }

    /// The names of this kind's options, in declaration order.
    pub fn option_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.options.iter().map(|(o, _)| *o)
    }
}

/// The literal an instance option takes (standard-library P3, #2342).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    /// A string literal: `@worker("legacy-billing")`.
    Str,
    /// A positive `Int` literal: `@limit(100)`.
    PositiveInt,
    /// A `Duration` literal: `@period(60.seconds)`.
    Duration,
}

/// The Worker binding prefix of a `Service` instance (interop I5, #1972):
/// `capability Billing = Service @worker("legacy-billing")` binds as
/// `SERVICE_BILLING`, a `[[services]]` entry naming `legacy-billing`.
pub const SERVICE_BINDING_PREFIX: &str = "SERVICE";

/// The option of a `Service` instance that names the Worker it reaches.
/// `worker`, not D4's `service`: `service` is a keyword, so `@service(…)`
/// does not parse, and widening the annotation grammar is language work
/// (ADR 0023).
pub const SERVICE_WORKER_OPTION: &str = "worker";

/// The Worker binding prefix of an `R2` instance (standard-library P5,
/// #2323): `capability Archives = R2` binds as `R2_ARCHIVES`, an
/// `[[r2_buckets]]` entry.
pub const R2_BINDING_PREFIX: &str = "R2";

/// The one creation option of an `R2` instance: the bucket's jurisdiction.
pub const R2_JURISDICTION_OPTION: &str = "jurisdiction";

/// The jurisdictions an `R2` bucket may be pinned to, as `@jurisdiction(…)`
/// takes them and `wrangler r2 bucket create --jurisdiction` accepts them.
pub const R2_JURISDICTIONS: &[&str] = &["eu", "fedramp"];

/// The Worker binding prefix of a `RateLimit` instance (standard-library P3,
/// #2342): `capability LoginLimit = RateLimit @limit(100) @period(60.seconds)`
/// binds as `RATELIMIT_LOGIN_LIMIT`, a `[[ratelimits]]` entry.
pub const RATELIMIT_BINDING_PREFIX: &str = "RATELIMIT";

/// A `RateLimit` instance's count: the calls per key allowed in one period.
pub const RATELIMIT_LIMIT_OPTION: &str = "limit";

/// A `RateLimit` instance's window.
pub const RATELIMIT_PERIOD_OPTION: &str = "period";

/// The windows the Workers Rate Limiting binding offers, in seconds.
pub const RATELIMIT_PERIODS: &[i64] = &[10, 60];

/// The Worker binding prefix of a `Queue` instance (standard-library P2,
/// #2578): `capability OrderEvents = Queue @name("order-events")` binds as
/// `QUEUE_ORDER_EVENTS`, a `[[queues.producers]]` entry.
pub const QUEUE_BINDING_PREFIX: &str = "QUEUE";

/// The option of a `Queue` instance that names the queue it sends to.
/// `name`, not the track's `queue`: `queue` is a keyword, so `@queue(…)` does
/// not parse (as `Service`'s `@worker`, ADR 0023).
pub const QUEUE_NAME_OPTION: &str = "name";

/// The Worker binding prefix of an `Analytics` instance (standard-library
/// P3b, #2582): `capability Signups = Analytics` binds as
/// `ANALYTICS_SIGNUPS`, an `[[analytics_engine_datasets]]` entry. It takes no
/// options: `bynk deploy` derives the dataset's name (track A12).
pub const ANALYTICS_BINDING_PREFIX: &str = "ANALYTICS";

/// The prefix an `EdgeCache` instance's binding name would carry (first-party
/// registry L7, #2414). Workers Cache is a Worker-wide `[cache]` setting with
/// no `env` binding, so the name is never emitted: the instance is built from
/// the entry's `ctx` instead.
pub const EDGE_CACHE_BINDING_PREFIX: &str = "EDGE_CACHE";

/// Whether a first-party item is under the 1.0 promise (track D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    /// Under the promise: a stable L0/L1 item changes only at a major release; a
    /// stable L2/L3 item may gain operations and, if extensible, variants.
    Stable,
    /// No promise: it may change or disappear in any release, and using it needs
    /// an `[unstable] allow` entry in `bynk.toml`.
    Experimental,
}

impl Unit {
    /// The binding module to emit for this unit on `platform`, as
    /// `(output path, source)`, or `None` for a commons.
    pub fn binding_for(&self, platform: Platform) -> Option<(&'static str, &'static str)> {
        match self.binding {
            Binding::None => None,
            Binding::PerPlatform => Some((
                platform.bynk_binding_filename(),
                platform.bynk_binding_source(),
            )),
            Binding::Fixed { filename, source } => Some((filename, source)),
            Binding::PerPlatformFiles {
                cloudflare,
                node,
                browser,
            } => Some(match platform {
                Platform::Cloudflare => cloudflare,
                Platform::Node => node,
                Platform::Browser => browser,
            }),
        }
    }
}

/// The standard library's first-party units. Dependencies between them are not
/// listed here: they are the `uses`/`consumes` lines of each unit's own source,
/// which the checker follows when it injects. The table's order is
/// [`FIRSTPARTY_SOURCES`]' order; injection follows [`Unit::id_block`].
pub const UNITS: &[Unit] = &[
    Unit {
        name: BYNK_UNIT,
        kind: UnitKind::Adapter,
        identity_path: "bynk.bynk",
        source: BYNK_ADAPTER_SRC,
        binding: Binding::PerPlatform,
        id_block: 0,
        platform: None,
        env_providers: &["SecretsProvider"],
        stability: Stability::Stable,
        // K5a (#2586): the new operations, until a service has used them.
        experimental: &["Fetch.sendWith", "Logger.log"],
        extensible: &["FetchError", "Method", "Redirect"],
        instantiable: &[],
    },
    Unit {
        name: CLOUDFLARE_UNIT,
        kind: UnitKind::Adapter,
        identity_path: "bynk/cloudflare.bynk",
        source: CLOUDFLARE_ADAPTER_SRC,
        binding: Binding::Fixed {
            filename: CLOUDFLARE_BINDING_FILENAME,
            source: CLOUDFLARE_BINDING,
        },
        id_block: 1,
        platform: Some(Platform::Cloudflare),
        env_providers: &[
            "WorkersKv",
            "WorkersService",
            "WorkersR2",
            "WorkersRateLimit",
            "WorkersQueue",
            "WorkersAnalytics",
        ],
        stability: Stability::Stable,
        // Interop I5 (#1972): a Service Binding to a non-Bynk Worker is new
        // and its surface is untyped, so it lands outside the 1.0 promise.
        // P5 (#2323): R2 ships experimental until the first-party registry has
        // run on it (#2308 R6); its conditions are extensible meanwhile.
        // P3 (#2342): RateLimit too, until the registry has run on it.
        // L7 (#2414): EdgeCache likewise.
        // P2 (#2578): Queue too, until a first-party service has sent on it.
        // P3b (#2582): Analytics too, until a first-party service writes to it.
        // P1 (#2590): the new operations of the stable `Kv`, until a service
        // has used them.
        experimental: &[
            "Service",
            "R2",
            "RateLimit",
            "EdgeCache",
            "Queue",
            "Analytics",
            "Kv.getWith",
            "Kv.putWith",
            "Kv.getBytes",
            "Kv.putBytes",
            "Kv.listPage",
        ],
        extensible: &["R2Condition", "KvExpiration"],
        instantiable: &[
            Instantiable {
                capability: "Kv",
                binding_prefix: KV_BINDING_NAME,
                options: &[],
                required: &[],
                bare: true,
            },
            Instantiable {
                capability: "Service",
                binding_prefix: SERVICE_BINDING_PREFIX,
                options: &[(SERVICE_WORKER_OPTION, OptionKind::Str)],
                required: &[SERVICE_WORKER_OPTION],
                bare: false,
            },
            Instantiable {
                capability: "R2",
                binding_prefix: R2_BINDING_PREFIX,
                options: &[(R2_JURISDICTION_OPTION, OptionKind::Str)],
                required: &[],
                bare: false,
            },
            Instantiable {
                capability: "RateLimit",
                binding_prefix: RATELIMIT_BINDING_PREFIX,
                options: &[
                    (RATELIMIT_LIMIT_OPTION, OptionKind::PositiveInt),
                    (RATELIMIT_PERIOD_OPTION, OptionKind::Duration),
                ],
                required: &[RATELIMIT_LIMIT_OPTION, RATELIMIT_PERIOD_OPTION],
                bare: false,
            },
            Instantiable {
                capability: "Queue",
                binding_prefix: QUEUE_BINDING_PREFIX,
                options: &[(QUEUE_NAME_OPTION, OptionKind::Str)],
                required: &[QUEUE_NAME_OPTION],
                bare: false,
            },
            Instantiable {
                capability: "Analytics",
                binding_prefix: ANALYTICS_BINDING_PREFIX,
                options: &[],
                required: &[],
                bare: false,
            },
            Instantiable {
                capability: "EdgeCache",
                binding_prefix: EDGE_CACHE_BINDING_PREFIX,
                options: &[],
                required: &[],
                bare: false,
            },
        ],
    },
    Unit {
        name: LIST_UNIT,
        kind: UnitKind::Commons,
        identity_path: "bynk/list.bynk",
        source: BYNK_LIST_SRC,
        binding: Binding::None,
        id_block: 3,
        platform: None,
        env_providers: &[],
        stability: Stability::Stable,
        experimental: &[],
        extensible: &[],
        instantiable: &[],
    },
    Unit {
        name: MAP_UNIT,
        kind: UnitKind::Commons,
        identity_path: "bynk/map.bynk",
        source: BYNK_MAP_SRC,
        binding: Binding::None,
        id_block: 2,
        platform: None,
        env_providers: &[],
        stability: Stability::Stable,
        experimental: &[],
        extensible: &[],
        instantiable: &[],
    },
    Unit {
        name: STRING_UNIT,
        kind: UnitKind::Commons,
        identity_path: "bynk/string.bynk",
        source: BYNK_STRING_SRC,
        binding: Binding::None,
        id_block: 4,
        platform: None,
        env_providers: &[],
        stability: Stability::Stable,
        experimental: &[],
        extensible: &[],
        instantiable: &[],
    },
    Unit {
        name: LOCALE_UNIT,
        kind: UnitKind::Commons,
        identity_path: "bynk/locale.bynk",
        source: BYNK_LOCALE_SRC,
        binding: Binding::None,
        id_block: 6,
        platform: None,
        env_providers: &[],
        stability: Stability::Stable,
        experimental: &[],
        extensible: &[],
        instantiable: &[],
    },
    Unit {
        name: LOCALE_TYPES_UNIT,
        kind: UnitKind::Commons,
        identity_path: "bynk/locale/types.bynk",
        source: BYNK_LOCALE_TYPES_SRC,
        binding: Binding::None,
        id_block: 5,
        platform: None,
        env_providers: &[],
        stability: Stability::Stable,
        experimental: &[],
        extensible: &[],
        instantiable: &[],
    },
    Unit {
        name: CRYPTO_UNIT,
        kind: UnitKind::Adapter,
        identity_path: "bynk/crypto.bynk",
        source: BYNK_CRYPTO_SRC,
        binding: Binding::PerPlatformFiles {
            cloudflare: (
                "bynk/crypto-cloudflare.binding.ts",
                CRYPTO_CLOUDFLARE_BINDING,
            ),
            node: ("bynk/crypto-node.binding.ts", CRYPTO_NODE_BINDING),
            browser: ("bynk/crypto-browser.binding.ts", CRYPTO_BROWSER_BINDING),
        },
        id_block: 7,
        platform: None,
        env_providers: &[],
        // Experimental until the first-party registry has signed with it in
        // production (#2308 R6), as P5's R2 does.
        stability: Stability::Experimental,
        experimental: &[],
        extensible: &["Digest", "Signature", "CryptoError"],
        instantiable: &[],
    },
    Unit {
        name: URL_UNIT,
        kind: UnitKind::Commons,
        identity_path: "bynk/url.bynk",
        source: BYNK_URL_SRC,
        binding: Binding::None,
        id_block: 8,
        platform: None,
        env_providers: &[],
        stability: Stability::Stable,
        experimental: &[],
        extensible: &[],
        instantiable: &[],
    },
];

/// The registry entry for `name`, if it is a first-party unit.
pub fn unit(name: &str) -> Option<&'static Unit> {
    UNITS.iter().find(|u| u.name == name)
}

/// The unit a dotted item path lies in, and the rest of the path relative to it
/// (empty for the unit itself). Unit names contain dots, so the longest unit
/// name that is a whole-segment prefix wins: `bynk.cloudflare.Kv` is in
/// `bynk.cloudflare`, not in `bynk`.
pub fn split_item_path(path: &str) -> Option<(&'static Unit, &str)> {
    split_item_path_in(UNITS, path)
}

/// [`split_item_path`] over a given table (tests pass a synthetic one).
pub fn split_item_path_in<'u, 'p>(units: &'u [Unit], path: &'p str) -> Option<(&'u Unit, &'p str)> {
    units
        .iter()
        .filter_map(|u| {
            if path == u.name {
                Some((u, ""))
            } else {
                path.strip_prefix(u.name)?
                    .strip_prefix('.')
                    .map(|rest| (u, rest))
            }
        })
        .max_by_key(|(u, _)| u.name.len())
}

/// Whether `path` is `prefix` or lies under it, by whole dotted segments.
pub fn covers(prefix: &str, path: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// The stability of a first-party item: a unit (`bynk.cloudflare`), an
/// exported item (`bynk.cloudflare.Kv`) or a capability operation
/// (`bynk.cloudflare.Kv.list`). An item in an experimental unit, or under an
/// experimental item, is experimental. `None` when no first-party unit holds
/// the path.
pub fn stability_of(path: &str) -> Option<Stability> {
    stability_of_in(UNITS, path)
}

/// [`stability_of`] over a given table.
pub fn stability_of_in(units: &[Unit], path: &str) -> Option<Stability> {
    let (unit, rest) = split_item_path_in(units, path)?;
    if unit.stability == Stability::Experimental
        || unit.experimental.iter().any(|item| covers(item, rest))
    {
        Some(Stability::Experimental)
    } else {
        Some(Stability::Stable)
    }
}

/// Every experimental item, as a full path: each experimental unit, and each
/// listed item of a stable one.
pub fn experimental_items() -> Vec<String> {
    experimental_items_in(UNITS)
}

/// [`experimental_items`] over a given table.
pub fn experimental_items_in(units: &[Unit]) -> Vec<String> {
    let mut out = Vec::new();
    for u in units {
        if u.stability == Stability::Experimental {
            out.push(u.name.to_string());
        }
        out.extend(
            u.experimental
                .iter()
                .map(|item| format!("{}.{item}", u.name)),
        );
    }
    out
}

/// Whether an `[unstable] allow` list covers `path`: an entry covers itself and
/// everything under it.
pub fn allows(allow: &[String], path: &str) -> bool {
    allow.iter().any(|entry| covers(entry, path))
}

/// Whether `entry`, an `[unstable] allow` entry, reaches anything in
/// `experimental` (a list of experimental item paths, such as
/// [`experimental_items`]): it names an experimental item, lies under one, or
/// contains one. An entry that reaches nothing is stale.
pub fn allow_entry_is_live(experimental: &[String], entry: &str) -> bool {
    experimental
        .iter()
        .any(|item| covers(item, entry) || covers(entry, item))
}

/// The instance kind `capability` of `unit` is, if a context may declare named
/// instances of it (S4a, #1966).
pub fn instantiable(unit: &str, capability: &str) -> Option<&'static Instantiable> {
    self::unit(unit)?
        .instantiable
        .iter()
        .find(|i| i.capability == capability)
}

/// Whether `name`, an enum exported by `unit`, is extensible.
pub fn is_extensible(unit: &str, name: &str) -> bool {
    is_extensible_in(UNITS, unit, name)
}

/// [`is_extensible`] over a given table.
pub fn is_extensible_in(units: &[Unit], unit: &str, name: &str) -> bool {
    units
        .iter()
        .any(|u| u.name == unit && u.extensible.contains(&name))
}

/// The names a first-party unit exports: those on its `exports … { … }` lines
/// (an adapter) and its own `fn`s and `type`s (a commons). Read from the
/// source, so the list cannot fall behind it.
pub fn exported_names(unit: &Unit) -> Vec<&'static str> {
    let mut out = Vec::new();
    let source = unit.source;
    let mut offset = 0;
    for line in source.lines() {
        let start = offset;
        offset += line.len() + 1;
        let trimmed = line.trim_start();
        // The list may run over several lines (`bynk-fmt` wraps a long one,
        // one name a line), so it is read from the source up to its `}`.
        if trimmed.starts_with("exports ")
            && let Some(open) = source[start..].find('{').map(|i| start + i)
            && let Some(close) = source[open..].find('}').map(|i| open + i)
        {
            out.extend(
                source[open + 1..close]
                    .split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty()),
            );
        } else if unit.kind == UnitKind::Commons
            // A commons' own declarations: at column 0 in the flat form, one tab
            // in the braced `commons bynk.list { … }` form the first-party
            // sources use. Nothing deeper is a declaration.
            && (line == trimmed || line.strip_prefix('\t') == Some(trimmed))
        {
            for keyword in ["fn ", "type "] {
                if let Some(rest) = trimmed.strip_prefix(keyword) {
                    let end = rest
                        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .unwrap_or(rest.len());
                    if end > 0 {
                        out.push(&rest[..end]);
                    }
                }
            }
        }
    }
    out
}

/// Whether `path` names a first-party unit or an item it exports. Operation
/// paths (`bynk.cloudflare.Kv.list`) are accepted when their item exists.
pub fn names_first_party_item(path: &str) -> bool {
    names_first_party_item_in(UNITS, path)
}

/// [`names_first_party_item`] over a given table.
pub fn names_first_party_item_in(units: &[Unit], path: &str) -> bool {
    match split_item_path_in(units, path) {
        None => false,
        Some((_, "")) => true,
        Some((unit, rest)) => {
            let item = rest.split('.').next().unwrap_or(rest);
            exported_names(unit).contains(&item)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic registry with an experimental unit, an experimental item
    /// and an experimental operation inside stable units.
    const SYNTH: &[Unit] = &[
        Unit {
            name: "bynk",
            kind: UnitKind::Adapter,
            identity_path: "bynk.bynk",
            source: "adapter bynk {\n\texports capability { Clock, Fetch }\n\texports transparent { FetchError }\n}\n",
            binding: Binding::PerPlatform,
            id_block: 0,
            platform: None,
            env_providers: &[],
            stability: Stability::Stable,
            experimental: &["Fetch.stream"],
            extensible: &["FetchError"],
            instantiable: &[],
        },
        Unit {
            name: "bynk.cloudflare",
            kind: UnitKind::Adapter,
            identity_path: "bynk/cloudflare.bynk",
            source: "adapter bynk.cloudflare {\n\texports capability { Kv, Ai }\n}\n",
            binding: Binding::None,
            id_block: 1,
            platform: Some(Platform::Cloudflare),
            env_providers: &[],
            stability: Stability::Stable,
            experimental: &["Ai"],
            extensible: &[],
            instantiable: &[],
        },
        Unit {
            name: "bynk.cloudflare.d1",
            kind: UnitKind::Adapter,
            identity_path: "bynk/cloudflare/d1.bynk",
            source: "adapter bynk.cloudflare.d1 {\n\texports capability { D1 }\n}\n",
            binding: Binding::None,
            id_block: 2,
            platform: Some(Platform::Cloudflare),
            env_providers: &[],
            stability: Stability::Experimental,
            experimental: &[],
            extensible: &[],
            instantiable: &[],
        },
        Unit {
            name: "bynk.text",
            kind: UnitKind::Commons,
            identity_path: "bynk/text.bynk",
            source: "commons bynk.text\n\ntype Slug = String\n\nfn shout(s: String) -> String {\n\ts\n}\n",
            binding: Binding::None,
            id_block: 3,
            platform: None,
            env_providers: &[],
            stability: Stability::Stable,
            experimental: &[],
            extensible: &[],
            instantiable: &[],
        },
    ];

    #[test]
    fn item_paths_resolve_to_the_longest_unit() {
        let unit_of = |p| split_item_path_in(SYNTH, p).map(|(u, rest)| (u.name, rest));
        assert_eq!(unit_of("bynk"), Some(("bynk", "")));
        assert_eq!(unit_of("bynk.Fetch.send"), Some(("bynk", "Fetch.send")));
        assert_eq!(
            unit_of("bynk.cloudflare.Kv"),
            Some(("bynk.cloudflare", "Kv"))
        );
        assert_eq!(
            unit_of("bynk.cloudflare.d1.D1"),
            Some(("bynk.cloudflare.d1", "D1"))
        );
        assert_eq!(unit_of("bynkx"), None, "a whole-segment prefix only");
        assert_eq!(unit_of("user.unit"), None);
    }

    #[test]
    fn stability_follows_unit_item_and_operation() {
        let st = |p| stability_of_in(SYNTH, p);
        use Stability::*;
        assert_eq!(st("bynk.Clock"), Some(Stable));
        assert_eq!(st("bynk.Fetch.send"), Some(Stable));
        assert_eq!(
            st("bynk.Fetch.stream"),
            Some(Experimental),
            "an experimental operation"
        );
        assert_eq!(
            st("bynk.Fetch"),
            Some(Stable),
            "its capability stays stable"
        );
        assert_eq!(
            st("bynk.cloudflare.Ai"),
            Some(Experimental),
            "an experimental item"
        );
        assert_eq!(
            st("bynk.cloudflare.Ai.run"),
            Some(Experimental),
            "its operations too"
        );
        assert_eq!(st("bynk.cloudflare.Kv"), Some(Stable));
        assert_eq!(
            st("bynk.cloudflare.d1"),
            Some(Experimental),
            "an experimental unit"
        );
        assert_eq!(st("bynk.cloudflare.d1.D1.query"), Some(Experimental));
        assert_eq!(st("user.unit"), None);
    }

    #[test]
    fn allow_entries_cover_by_whole_segment_prefix() {
        let allow = vec![
            "bynk.cloudflare.Ai".to_string(),
            "bynk.cloudflare.d1".to_string(),
        ];
        assert!(allows(&allow, "bynk.cloudflare.Ai"));
        assert!(allows(&allow, "bynk.cloudflare.Ai.run"));
        assert!(allows(&allow, "bynk.cloudflare.d1.D1.query"));
        assert!(
            !allows(&allow, "bynk.cloudflare.AiX"),
            "not a segment prefix"
        );
        assert!(!allows(&allow, "bynk.Fetch.stream"));
        assert!(!allows(&[], "bynk.cloudflare.Ai"));
    }

    #[test]
    fn an_allow_entry_is_live_while_it_reaches_something_experimental() {
        let synth = experimental_items_in(SYNTH);
        let live = |e| allow_entry_is_live(&synth, e);
        assert!(live("bynk.cloudflare.Ai"));
        assert!(live("bynk.cloudflare.Ai.run"), "under an experimental item");
        assert!(live("bynk.cloudflare"), "contains an experimental item");
        assert!(live("bynk.Fetch.stream"));
        assert!(live("bynk"), "contains an experimental operation");
        assert!(!live("bynk.cloudflare.Kv"), "stable: a stale entry");
        assert!(!live("bynk.text"));
        assert_eq!(
            experimental_items_in(SYNTH),
            [
                "bynk.Fetch.stream",
                "bynk.cloudflare.Ai",
                "bynk.cloudflare.d1"
            ]
        );
    }

    #[test]
    fn exported_names_and_item_existence_come_from_the_sources() {
        assert_eq!(exported_names(&SYNTH[0]), ["Clock", "Fetch", "FetchError"]);
        // K5a (#2586): a list `bynk-fmt` wrapped, one name a line.
        let wrapped = Unit {
            source: "adapter bynk {\n\texports transparent {\n\t\tMethod,\n\t\tRedirect,\n\t}\n\texports capability { Fetch }\n}\n",
            ..SYNTH[0]
        };
        assert_eq!(exported_names(&wrapped), ["Method", "Redirect", "Fetch"]);
        assert_eq!(exported_names(&SYNTH[3]), ["Slug", "shout"]);
        let names = |p| names_first_party_item_in(SYNTH, p);
        assert!(names("bynk.cloudflare"));
        assert!(names("bynk.cloudflare.Ai"));
        assert!(
            names("bynk.cloudflare.Ai.run"),
            "an operation of an existing item"
        );
        assert!(names("bynk.text.shout"));
        assert!(!names("bynk.cloudflare.Aii"), "a typo");
        assert!(!names("user.unit"));
    }

    #[test]
    fn extensibility_is_by_unit_and_name() {
        assert!(is_extensible_in(SYNTH, "bynk", "FetchError"));
        assert!(
            !is_extensible_in(SYNTH, "user.app", "FetchError"),
            "a user's own FetchError"
        );
        assert!(!is_extensible_in(SYNTH, "bynk", "Method"));
    }

    /// The shipped instantiable kinds: `bynk.cloudflare`'s `Kv` (S4a, #1966),
    /// binding as `KV_<NAME>` with no creation options, and its `Service`
    /// (interop I5, #1972), binding as `SERVICE_<NAME>` and naming its Worker
    /// with a required `@worker`, never usable bare. Each one is a capability
    /// its unit exports.
    #[test]
    fn the_shipped_instantiable_kinds_are_pinned() {
        let kinds: Vec<(&str, &str, &str)> = UNITS
            .iter()
            .flat_map(|u| {
                u.instantiable
                    .iter()
                    .map(move |i| (u.name, i.capability, i.binding_prefix))
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("bynk.cloudflare", "Kv", "KV"),
                ("bynk.cloudflare", "Service", "SERVICE"),
                ("bynk.cloudflare", "R2", "R2"),
                ("bynk.cloudflare", "RateLimit", "RATELIMIT"),
                ("bynk.cloudflare", "Queue", "QUEUE"),
                ("bynk.cloudflare", "Analytics", "ANALYTICS"),
                ("bynk.cloudflare", "EdgeCache", "EDGE_CACHE")
            ]
        );
        let kv = instantiable("bynk.cloudflare", "Kv").expect("Kv is instantiable");
        assert!(kv.options.is_empty() && kv.required.is_empty() && kv.bare);
        let service = instantiable("bynk.cloudflare", "Service").expect("Service is instantiable");
        assert_eq!(service.option_names().collect::<Vec<_>>(), ["worker"]);
        assert_eq!(service.required, ["worker"]);
        assert!(!service.bare);
        let r2 = instantiable("bynk.cloudflare", "R2").expect("R2 is instantiable");
        assert_eq!(r2.option_names().collect::<Vec<_>>(), ["jurisdiction"]);
        assert!(r2.required.is_empty() && !r2.bare);
        let rl = instantiable("bynk.cloudflare", "RateLimit").expect("RateLimit is instantiable");
        assert_eq!(rl.option_kind("limit"), Some(OptionKind::PositiveInt));
        assert_eq!(rl.option_kind("period"), Some(OptionKind::Duration));
        assert_eq!(rl.required, ["limit", "period"]);
        assert!(!rl.bare);
        let queue = instantiable("bynk.cloudflare", "Queue").expect("Queue is instantiable");
        assert_eq!(queue.option_kind("name"), Some(OptionKind::Str));
        assert_eq!(queue.required, ["name"]);
        assert!(!queue.bare);
        let analytics =
            instantiable("bynk.cloudflare", "Analytics").expect("Analytics is instantiable");
        assert!(analytics.options.is_empty() && !analytics.bare);
        let edge = instantiable("bynk.cloudflare", "EdgeCache").expect("EdgeCache is instantiable");
        assert!(edge.options.is_empty() && !edge.bare);
        for u in UNITS {
            for i in u.instantiable {
                assert!(
                    i.required.iter().all(|r| i.option_kind(r).is_some()),
                    "{}.{}: a required option must be an option",
                    u.name,
                    i.capability
                );
            }
        }
        assert!(exported_names(unit("bynk.cloudflare").unwrap()).contains(&"Kv"));
        assert!(exported_names(unit("bynk.cloudflare").unwrap()).contains(&"Service"));
        assert_eq!(instantiable("bynk", "Clock"), None);
        assert_eq!(instantiable("bynk.cloudflare", "Nope"), None);
    }

    /// The shipped registry: every unit is stable (S3 ships the mechanism,
    /// #1963 Decision C) but `bynk.crypto` (K4), which is experimental until the
    /// first-party registry has signed with it in production. The one
    /// experimental item inside a stable unit is `bynk.cloudflare`'s `Service`
    /// (interop I5, #1972), and the growing enums are extensible. A slice that
    /// marks an item experimental updates this test, so the change is visible
    /// in review.
    #[test]
    fn the_shipped_stability_is_pinned() {
        for u in UNITS {
            let expected = if u.name == CRYPTO_UNIT {
                Stability::Experimental
            } else {
                Stability::Stable
            };
            assert_eq!(u.stability, expected, "{}", u.name);
        }
        assert_eq!(
            experimental_items(),
            [
                "bynk.Fetch.sendWith",
                "bynk.Logger.log",
                "bynk.cloudflare.Service",
                "bynk.cloudflare.R2",
                "bynk.cloudflare.RateLimit",
                "bynk.cloudflare.EdgeCache",
                "bynk.cloudflare.Queue",
                "bynk.cloudflare.Analytics",
                "bynk.cloudflare.Kv.getWith",
                "bynk.cloudflare.Kv.putWith",
                "bynk.cloudflare.Kv.getBytes",
                "bynk.cloudflare.Kv.putBytes",
                "bynk.cloudflare.Kv.listPage",
                "bynk.crypto"
            ]
        );
        let extensible: Vec<(&str, &str)> = UNITS
            .iter()
            .flat_map(|u| u.extensible.iter().map(move |e| (u.name, *e)))
            .collect();
        assert_eq!(
            extensible,
            [
                ("bynk", "FetchError"),
                ("bynk", "Method"),
                ("bynk", "Redirect"),
                ("bynk.cloudflare", "R2Condition"),
                ("bynk.cloudflare", "KvExpiration"),
                ("bynk.crypto", "Digest"),
                ("bynk.crypto", "Signature"),
                ("bynk.crypto", "CryptoError"),
            ]
        );
        for (unit, name) in extensible {
            assert!(
                exported_names(self::unit(unit).unwrap()).contains(&name),
                "{unit} lists {name} as extensible but does not export it"
            );
        }
        assert!(names_first_party_item("bynk.cloudflare.Kv.list"));
        assert!(names_first_party_item("bynk.list.reverse"));
        assert!(!names_first_party_item("bynk.cloudflare.Kvv"));
    }

    /// #1958: `WorkersKv` reads the namespace at the name every generated
    /// Worker declares it under (`[[kv_namespaces]] binding`, its `Env`), the
    /// one TypeScript literal no Rust constructor can supply. Since S4a (#1966)
    /// the name is a constructor parameter, so a derived capability's instance
    /// can pass its own; the bare `Kv`'s is the parameter's default.
    ///
    /// Matched as a whole identifier, then by its neighbours, so the test
    /// ignores formatting but not a rename: `KVX` is not `KV` (review of
    /// #1962).
    #[test]
    fn the_cloudflare_binding_reads_kv_binding_name() {
        let ident = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
        let src = CLOUDFLARE_BINDING;
        let uses: Vec<(&str, &str)> = src
            .match_indices(KV_BINDING_NAME)
            .map(|(at, name)| (&src[..at], &src[at + name.len()..]))
            .filter(|(before, after)| !before.ends_with(ident) && !after.starts_with(ident))
            .collect();
        assert!(
            uses.iter().any(|(b, a)| {
                b.trim_end().ends_with("binding: string = \"") && a.starts_with('"')
            }),
            "the constructor's `binding` no longer defaults to \"{KV_BINDING_NAME}\""
        );
        assert!(
            src.contains("?.[this.binding]"),
            "the namespace is no longer read at `env[binding]`"
        );
    }

    /// Each unit's `ExprId` block is permanent: emitted output and the
    /// process-wide parse cache depend on it. These are the blocks the
    /// hand-written injection used before the registry (S1, #1945); a new
    /// unit takes a new block and never moves one of these.
    #[test]
    fn id_blocks_are_pinned_and_distinct() {
        let blocks: Vec<(&str, u32)> = UNITS.iter().map(|u| (u.name, u.id_block)).collect();
        assert_eq!(
            blocks,
            [
                ("bynk", 0),
                ("bynk.cloudflare", 1),
                ("bynk.list", 3),
                ("bynk.map", 2),
                ("bynk.string", 4),
                ("bynk.locale", 6),
                ("bynk.locale.types", 5),
                ("bynk.crypto", 7),
                ("bynk.url", 8),
            ]
        );
        let mut sorted: Vec<u32> = UNITS.iter().map(|u| u.id_block).collect();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), UNITS.len(), "two units share an id block");
    }

    /// Hover and completion read [`FIRSTPARTY_SOURCES`] in order, so it is
    /// pinned to the order the hand-written list had.
    #[test]
    fn firstparty_sources_keep_their_order() {
        let names: Vec<&str> = FIRSTPARTY_SOURCES.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "bynk",
                "bynk.cloudflare",
                "bynk.list",
                "bynk.map",
                "bynk.string",
                "bynk.locale",
                "bynk.locale.types",
                "bynk.crypto",
                "bynk.url",
            ]
        );
        for (unit, (name, source)) in UNITS.iter().zip(FIRSTPARTY_SOURCES) {
            assert_eq!((unit.name, unit.source), (*name, *source));
        }
    }

    /// The views over the table answer as the hand-written matches did.
    #[test]
    fn metadata_views_match_the_previous_answers() {
        assert_eq!(platform_of("bynk.cloudflare"), Some(Platform::Cloudflare));
        for name in [
            "bynk",
            "bynk.list",
            "bynk.map",
            "bynk.string",
            "bynk.locale",
            "user.unit",
        ] {
            assert_eq!(platform_of(name), None, "{name}");
        }
        assert!(provider_takes_env("bynk", "SecretsProvider"));
        assert!(provider_takes_env("bynk.cloudflare", "WorkersKv"));
        assert!(!provider_takes_env("bynk", "WorkersKv"));
        assert!(!provider_takes_env("bynk.cloudflare", "SecretsProvider"));
        assert!(!provider_takes_env("bynk", "ClockProvider"));
        assert!(!provider_takes_env("user.unit", "SecretsProvider"));
    }

    /// An adapter carries a binding and a commons does not, and every name and
    /// identity path is unique, every name inside the reserved prefix.
    #[test]
    fn entries_are_well_formed() {
        for u in UNITS {
            assert!(
                u.name == "bynk" || u.name.starts_with("bynk."),
                "{}",
                u.name
            );
            assert_eq!(
                UNITS.iter().filter(|o| o.name == u.name).count(),
                1,
                "{}",
                u.name
            );
            // Two entries under one identity path would be two parsed units
            // that diagnostics and emission cannot tell apart.
            assert_eq!(
                UNITS
                    .iter()
                    .filter(|o| o.identity_path == u.identity_path)
                    .count(),
                1,
                "{}",
                u.identity_path
            );
            match u.kind {
                UnitKind::Adapter => assert_ne!(u.binding, Binding::None, "{}", u.name),
                UnitKind::Commons => {
                    assert_eq!(u.binding, Binding::None, "{}", u.name);
                    assert!(u.env_providers.is_empty(), "{}", u.name);
                }
            }
        }
        assert_eq!(
            unit("bynk.cloudflare").and_then(|u| u.binding_for(Platform::Node)),
            Some((CLOUDFLARE_BINDING_FILENAME, CLOUDFLARE_BINDING))
        );
        assert_eq!(
            unit("bynk").and_then(|u| u.binding_for(Platform::Browser)),
            Some(("bynk-browser.ts", BYNK_BROWSER_BINDING))
        );
        assert_eq!(
            unit("bynk.list").and_then(|u| u.binding_for(Platform::Node)),
            None
        );
    }

    /// The three `bynk.crypto` bindings are one body with one marked region of
    /// platform difference (the constant-time comparison primitive). They are
    /// separate files because each ships alone, so this keeps them from drifting.
    #[test]
    fn the_crypto_bindings_differ_only_in_their_platform_region() {
        const START: &str = "// --- platform: constant-time comparison primitive";
        const END: &str = "// --- end platform";
        fn shared(src: &str) -> String {
            let (head, rest) = src.split_once(START).expect("platform region start");
            let (_, tail) = rest.split_once(END).expect("platform region end");
            // The second header line names the platform.
            let head = head.lines().skip(2).collect::<Vec<_>>().join("\n");
            format!("{head}\n{tail}")
        }
        let cloudflare = shared(CRYPTO_CLOUDFLARE_BINDING);
        assert_eq!(cloudflare, shared(CRYPTO_NODE_BINDING));
        assert_eq!(cloudflare, shared(CRYPTO_BROWSER_BINDING));
    }

    /// The hardcoded [`BYNK_SURFACE_CAPABILITIES`] must match the adapter's
    /// `exports capability { … }` line — otherwise the unknown-name quick-fix
    /// (#852) would offer a stale `consumes bynk { … }` set.
    #[test]
    fn bynk_surface_capabilities_match_the_adapter() {
        let src = BYNK_ADAPTER_SRC;
        let line = src
            .lines()
            .find(|l| l.trim_start().starts_with("exports capability {"))
            .expect("adapter declares `exports capability`");
        let inner = line
            .split_once('{')
            .and_then(|(_, r)| r.split_once('}'))
            .map(|(names, _)| names)
            .expect("braced capability list");
        let declared: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
        assert_eq!(
            declared, BYNK_SURFACE_CAPABILITIES,
            "BYNK_SURFACE_CAPABILITIES is out of sync with bynk.bynk"
        );
    }
}

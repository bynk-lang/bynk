//! Registries (packaging slice 4a, #2274; track `design/tracks/packaging.md`
//! §6.1–§6.2): the client `bynk` resolves `{ package, version }` dependencies
//! with, and the reference tooling for a static registry.
//!
//! The `Client` opens the registry an organisation routes to — through a
//! mirror, when one serves it — decides which key it trusts (a key pinned in
//! `[registries]`, else the one `bynk.lock` recorded, else trust on first
//! use), follows the key-rotation chain from it, and from then on admits only
//! index lines signed by a key on that chain and bound to the registry's `id`
//! and the package's name. Archives are unpacked into staging and admitted to
//! the store only when their tree checksum is the one the signed line names.

pub mod account;
pub(crate) mod archive;
pub mod commands;
pub mod conformance;
pub mod protocol;
pub mod publish;
pub(crate) mod select;
pub mod serve;
pub(crate) mod transport;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bynk_project::RegistryConfig;
use bynk_project::lock::{FIRST_PARTY, FIRST_PARTY_REGISTRY, Lock, LockedRegistry, store_dir};

use crate::fetch::{FetchError, fail};
use protocol::{Config, ConfigTrust, IndexLine, fingerprint, verify};
use transport::{Transport, join};

pub(crate) const BAD_SIGNATURE: &str = "bynk.registry.bad_signature";
pub(crate) const KEY_MISMATCH: &str = "bynk.registry.key_mismatch";
pub(crate) const UNAVAILABLE: &str = "bynk.registry.unavailable";
pub(crate) const SOURCE_MISMATCH: &str = "bynk.registry.source_mismatch";
pub(crate) const WITHDRAWN: &str = "bynk.registry.withdrawn";

/// A registry, opened and trusted.
struct Opened {
    /// The index this client reads from (a mirror's, when one serves it).
    index: String,
    /// #2330 (C): the mirror the index is read through, when one serves it —
    /// its unsigned fields are the mirror's word, not the origin's.
    mirror: Option<String>,
    /// The archive base.
    dl: String,
    /// #2287: the publish API's base, when the registry has one.
    api: Option<String>,
    /// The origin registry's `id`, which every line must bind.
    id: String,
    /// The keys a line may be signed by: the trusted key, every key the
    /// rotation chain reaches from it, and every valid, unrevoked delegate.
    keys: Vec<String>,
    /// #2316: delegates a valid revocation names — a line signed by one is
    /// refused, and the message says why.
    revoked: Vec<String>,
    transport: Transport,
    /// R3.2 (#2383, E): where the token the transport carries came from.
    token_source: Option<TokenSource>,
    /// Index files already read, by package.
    lines: BTreeMap<String, Vec<IndexLine>>,
}

/// The registries one resolution uses.
pub(crate) struct Client {
    /// The root's (or workspace's) `[registries]`.
    configs: Vec<RegistryConfig>,
    /// The trust records the existing lock holds.
    locked: Vec<LockedRegistry>,
    opened: BTreeMap<String, Opened>,
    /// The trust records this resolution used, for the lock.
    pub trust: BTreeMap<String, LockedRegistry>,
    /// One-line notices (trust on first use, a rotated key, credentials).
    pub notices: Vec<String>,
    /// R3.2 (#2383, B): the organisation a `bynk publish` or `bynk yank`
    /// acts for, whose own token slot is preferred.
    acting_for: Option<String>,
}

impl Client {
    pub(crate) fn new(configs: Vec<RegistryConfig>, existing: Option<&Lock>) -> Client {
        Client {
            configs,
            locked: existing.map(|l| l.registries.clone()).unwrap_or_default(),
            opened: BTreeMap::new(),
            trust: BTreeMap::new(),
            notices: Vec::new(),
            acting_for: None,
        }
    }

    /// R3.2 (#2383, B): act for `org` — every registry this client opens
    /// carries `org`'s own publish token, when one is saved.
    pub(crate) fn acting_for(mut self, org: &str) -> Client {
        self.acting_for = Some(org.to_string());
        self
    }

    /// R3.2 (#2383, E): where `registry`'s token came from, once opened.
    pub(crate) fn token_source(&self, registry: &str) -> Option<&TokenSource> {
        self.opened.get(registry)?.token_source.as_ref()
    }

    /// The registry `org` routes to: the one whose `organisations` lists it,
    /// else the first-party registry.
    pub(crate) fn route(&self, org: &str) -> String {
        self.configs
            .iter()
            .find(|c| c.organisations.iter().any(|o| o == org))
            .map_or_else(|| FIRST_PARTY_REGISTRY.to_string(), |c| c.name.clone())
    }

    /// Open `name`, deciding its trusted key.
    fn open(&mut self, name: &str) -> Result<&mut Opened, FetchError> {
        if self.opened.contains_key(name) {
            return Ok(self.opened.get_mut(name).expect("checked"));
        }
        // The first-party registry isn't live until its key ships: nothing —
        // not even a mirror of it — may be trusted for it on first use.
        if name == FIRST_PARTY_REGISTRY && FIRST_PARTY.is_none() {
            return Err(fail(
                UNAVAILABLE,
                "the first-party registry isn't live yet: add a `[registries.<name>]` whose `organisations` lists the organisation",
            ));
        }
        // Where to read it from: a mirror of it, else itself.
        let mirror = self
            .configs
            .iter()
            .find(|c| c.mirror_of.as_deref() == Some(name))
            .cloned();
        let own = self.configs.iter().find(|c| c.name == name).cloned();
        let (index, pinned, reader) = match (&mirror, &own) {
            (Some(m), _) => (
                m.index.clone(),
                own.as_ref()
                    .and_then(|o| o.key.clone())
                    .or_else(|| first_party_key(name)),
                m.name.clone(),
            ),
            (None, Some(o)) => (o.index.clone(), o.key.clone(), o.name.clone()),
            (None, None) if name == FIRST_PARTY_REGISTRY => match FIRST_PARTY {
                Some((index, key)) => (index.to_string(), Some(key.to_string()), name.to_string()),
                None => {
                    return Err(fail(
                        UNAVAILABLE,
                        "the first-party registry isn't live yet: add a `[registries.<name>]` whose `organisations` lists the organisation",
                    ));
                }
            },
            (None, None) => {
                return Err(fail(
                    UNAVAILABLE,
                    format!("no `[registries.{name}]` is configured"),
                ));
            }
        };
        let config_url = join(&index, "config.json");
        // A token the user supplied goes with every request to the registry's
        // origin — a private registry protects its `config.json` too.
        let (token, token_source) =
            token_for(&reader, self.acting_for.as_deref(), &mut self.notices).unzip();
        let mut transport = Transport::new(&reader, &[&index]).with_token(token);
        let bytes = transport.get(&config_url)?.ok_or_else(|| {
            fail(
                UNAVAILABLE,
                format!("registry `{reader}` has no `config.json` at `{config_url}`"),
            )
        })?;
        let config = Config::parse(&bytes)
            .map_err(|e| fail(UNAVAILABLE, format!("registry `{reader}`: {e}")))?;
        let locked = self.locked.iter().find(|r| r.name == name).cloned();
        // The origin's id: the lock's record, else (first use) what it says.
        let id = match &locked {
            Some(r) => {
                if mirror.is_none() && r.id != config.id {
                    return Err(fail(
                        KEY_MISMATCH,
                        format!(
                            "registry `{name}` now calls itself `{}`, but `bynk.lock` trusted it as `{}`",
                            config.id, r.id
                        ),
                    ));
                }
                r.id.clone()
            }
            None => config.id.clone(),
        };
        let trusted = match (&pinned, &locked) {
            (Some(key), _) => fingerprint(key).ok_or_else(|| {
                fail(
                    KEY_MISMATCH,
                    format!("`[registries.{name}] key` isn't an `ed25519:` key"),
                )
            })?,
            (None, Some(r)) => r.key.clone(),
            (None, None) => {
                let newest = &config.keys.last().expect("parse checks").key;
                let fp = fingerprint(newest).ok_or_else(|| {
                    fail(
                        KEY_MISMATCH,
                        format!("registry `{name}`'s key isn't an `ed25519:` key"),
                    )
                })?;
                self.notices.push(format!(
                    "trusting registry `{name}` (id `{}`) key {fp} on first use — pin it with `key = \"{newest}\"` in `[registries.{name}]`",
                    config.id
                ));
                fp
            }
        };
        let start = config
            .keys
            .iter()
            .position(|k| fingerprint(&k.key).as_deref() == Some(trusted.as_str()));
        let chained = config.follow_chain(&trusted).is_some();
        let (Some(start), true) = (start, chained) else {
            return Err(fail(
                KEY_MISMATCH,
                format!(
                    "registry `{name}`'s keys don't lead from the trusted key {trusted} — if its key was replaced deliberately, pin the new one in `[registries.{name}]`"
                ),
            ));
        };
        // The lock records the trust *anchor* — the key first trusted (or
        // pinned) — and every key the chain reaches from it is accepted. A
        // planned rotation therefore changes nothing in the lock, and lines
        // signed before it still verify.
        self.trust.insert(
            name.to_string(),
            LockedRegistry {
                name: name.to_string(),
                id: id.clone(),
                key: trusted.clone(),
            },
        );
        let dl = transport::resolve(&config_url, &config.dl);
        // A registry's `config.json` can't send the client somewhere its
        // `[registries]` entry didn't: a local registry keeps local archives,
        // and a remote one remote archives over a scheme at least as strong.
        let allowed = matches!(
            (scheme(&index).as_deref(), scheme(&dl).as_deref()),
            (Some("file"), Some("file"))
                | (Some("https"), Some("https"))
                | (Some("http"), Some("http" | "https"))
        );
        if !allowed {
            return Err(fail(
                UNAVAILABLE,
                format!(
                    "registry `{reader}`'s `config.json` puts its archives at `{dl}`, which its index `{index}` doesn't allow"
                ),
            ));
        }
        transport.allow_origin(&dl);
        // #2287: the publish API obeys the same scheme rule as `dl`.
        let api = match &config.api {
            None => None,
            Some(a) => {
                let api = transport::resolve(&config_url, a);
                let allowed = matches!(
                    (scheme(&index).as_deref(), scheme(&api).as_deref()),
                    (Some("https"), Some("https")) | (Some("http"), Some("http" | "https"))
                );
                if !allowed {
                    return Err(fail(
                        UNAVAILABLE,
                        format!(
                            "registry `{reader}`'s `config.json` puts its publish API at `{api}`, which its index `{index}` doesn't allow"
                        ),
                    ));
                }
                transport.allow_origin(&api);
                Some(api)
            }
        };
        // #2316 (B): the chain keys reached from the anchor may sign lines,
        // and so may each delegate a chain key delegated and none revoked.
        let chain: Vec<String> = config.keys[start..].iter().map(|k| k.key.clone()).collect();
        let signers = config.signers(&chain);
        for why in &signers.ignored {
            self.notices.push(format!(
                "registry `{name}`'s `config.json` has {why} — ignoring it"
            ));
        }
        let opened = Opened {
            index,
            mirror: mirror.map(|m| m.name),
            dl,
            api,
            id,
            keys: signers.keys,
            revoked: signers.revoked,
            transport,
            token_source,
            lines: BTreeMap::new(),
        };
        Ok(self.opened.entry(name.to_string()).or_insert(opened))
    }

    /// `package`'s verified index lines on `registry`.
    pub(crate) fn lines(
        &mut self,
        registry: &str,
        package: &str,
    ) -> Result<Vec<IndexLine>, FetchError> {
        let opened = self.open(registry)?;
        if let Some(lines) = opened.lines.get(package) {
            return Ok(lines.clone());
        }
        let url = join(&opened.index, package);
        let Some(bytes) = opened.transport.get(&url)? else {
            return Err(fail(
                UNAVAILABLE,
                format!("registry `{registry}` has no package `{package}`"),
            ));
        };
        let text = String::from_utf8(bytes).map_err(|_| {
            fail(
                BAD_SIGNATURE,
                format!("`{package}`'s index on `{registry}` isn't UTF-8"),
            )
        })?;
        let mut lines = Vec::new();
        for raw in text.lines().filter(|l| !l.trim().is_empty()) {
            let line = IndexLine::parse(raw)
                .map_err(|e| fail(BAD_SIGNATURE, format!("`{package}` on `{registry}`: {e}")))?;
            let payload = line.payload(&opened.id);
            let signed = opened.keys.iter().any(|k| verify(k, &payload, &line.sig));
            if !signed
                && line.name == package
                && let Some(k) = opened
                    .revoked
                    .iter()
                    .find(|k| verify(k, &payload, &line.sig))
            {
                return Err(fail(
                    BAD_SIGNATURE,
                    format!(
                        "`{package}` {} on registry `{registry}` is signed by {k}, a key the registry revoked — the registry must re-sign it (`bynk registry resign`)",
                        line.version
                    ),
                ));
            }
            if !signed || line.name != package {
                return Err(fail(
                    BAD_SIGNATURE,
                    format!(
                        "`{package}` {} on registry `{registry}` isn't signed by its trusted key for `{package}` on `{}` — refusing it",
                        line.version, opened.id
                    ),
                ));
            }
            lines.push(line);
        }
        opened.lines.insert(package.to_string(), lines.clone());
        Ok(lines)
    }

    /// #2330 (C): refuse a write to `registry` when a mirror serves it. The
    /// mirror's `config.json` names the `api`, so the upload (and the mirror's
    /// token) would go where the mirror says — and the read-back would come
    /// through the mirror too. Decided from `[registries]` alone, before
    /// anything is fetched.
    pub(crate) fn writable(&self, registry: &str) -> Result<(), FetchError> {
        match self
            .configs
            .iter()
            .find(|c| c.mirror_of.as_deref() == Some(registry))
        {
            Some(mirror) => Err(fail(
                publish::PUBLISH_REFUSED,
                format!(
                    "registry `{registry}` is read through the mirror `{}`, and a publish or yank goes only to the registry itself — run this from a project without the mirror",
                    mirror.name
                ),
            )),
            None => Ok(()),
        }
    }

    /// #2287: a write to `registry`'s publish API at `path` (relative to its
    /// `api`): `PUT` with `body`, or `POST`. A registry with no `api` takes no
    /// publishes over HTTP, and one read through a mirror takes none at all.
    pub(crate) fn api_write(
        &mut self,
        registry: &str,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>), FetchError> {
        self.writable(registry)?;
        let opened = self.open(registry)?;
        let Some(api) = opened.api.clone() else {
            return Err(fail(
                publish::PUBLISH_REFUSED,
                format!(
                    "registry `{registry}` doesn't take publishes over HTTP (its `config.json` has no `api`) — publish with `bynk registry add`"
                ),
            ));
        };
        opened.transport.send(method, &join(&api, path), body)
    }

    /// R3.1 (#2378, decision B): a request to `registry`'s account API at
    /// `path`, which is the `account` sibling of its verified `api`, carrying
    /// `bearer`. A registry read through a mirror is refused: the mirror's
    /// `config.json` names the `api`, and an account token goes only to the
    /// origin.
    pub(crate) fn account_call(
        &mut self,
        registry: &str,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        bearer: Option<&str>,
    ) -> Result<(u16, Vec<u8>), FetchError> {
        let opened = self.open(registry)?;
        if let Some(mirror) = &opened.mirror {
            return Err(fail(
                account::ACCOUNT_REFUSED,
                format!(
                    "registry `{registry}` is read through the mirror `{mirror}`, and an account token goes only to the registry itself — run this from a project without the mirror"
                ),
            ));
        }
        let Some(api) = opened.api.clone() else {
            return Err(fail(
                account::ACCOUNT_REFUSED,
                format!(
                    "registry `{registry}` has no account API (its `config.json` has no `api`)"
                ),
            ));
        };
        let url = join(&account_base(&api), path);
        opened.transport.json(method, &url, body, bearer)
    }

    /// Forget `package`'s cached index lines on `registry`, so the next read
    /// is fresh.
    pub(crate) fn forget(&mut self, registry: &str, package: &str) {
        if let Some(o) = self.opened.get_mut(registry) {
            o.lines.remove(package);
        }
    }

    /// #2330 (B, C): the failure for a locked version the registry withdrew,
    /// naming the reason (sanitised), the way off it, and — when the line was
    /// read through a mirror — that the origin wasn't consulted.
    pub(crate) fn withdrawn(
        &self,
        registry: &str,
        package: &str,
        version: &str,
        reason: &str,
        alias: &str,
    ) -> FetchError {
        let mut message = format!(
            "`{package}` {version} was withdrawn by registry `{registry}`: {}\n  = note: `bynk.lock` locks this version; choose another with `bynk update {alias}`",
            sanitise_reason(reason)
        );
        if let Some(mirror) = self.opened.get(registry).and_then(|o| o.mirror.as_deref()) {
            message.push_str(&format!(
                "\n  = note: read through the mirror `{mirror}` — the origin wasn't consulted"
            ));
        }
        fail(WITHDRAWN, message)
    }

    /// Fetch `line`'s archive into the store unless its tree is there,
    /// admitting it only if its tree checksum is the signed one.
    pub(crate) fn fetch(
        &mut self,
        registry: &str,
        line: &IndexLine,
        home: &Path,
    ) -> Result<PathBuf, FetchError> {
        let entry = store_dir(home, &line.checksum).ok_or_else(|| {
            fail(
                BAD_SIGNATURE,
                format!("`{}`'s checksum is malformed", line.name),
            )
        })?;
        if entry.is_dir() {
            return Ok(entry);
        }
        let opened = self.open(registry)?;
        let hex = entry
            .file_name()
            .expect("a store path")
            .to_string_lossy()
            .into_owned();
        let url = join(&opened.dl, &format!("sha256/{hex}"));
        let bytes = opened.transport.get(&url)?.ok_or_else(|| {
            fail(
                UNAVAILABLE,
                format!(
                    "registry `{registry}` has no archive for `{}` {}",
                    line.name, line.version
                ),
            )
        })?;
        let staging = crate::fetch::staging_dir(home, &hex);
        std::fs::create_dir_all(&staging).map_err(|e| {
            fail(
                archive::BAD_ARCHIVE,
                format!("could not stage `{}`: {e}", line.name),
            )
        })?;
        archive::unpack(&bytes, &staging)?;
        let got = crate::fetch::place_in_store(
            home,
            &staging,
            &format!("`{}` {}", line.name, line.version),
            archive::BAD_ARCHIVE,
        )?;
        if got != line.checksum {
            if let Some(wrong) = store_dir(home, &got) {
                let _ = crate::fetch::remove_entry(&wrong);
            }
            return Err(fail(
                crate::fetch::CHECKSUM_MISMATCH,
                format!(
                    "`{}` {}'s archive from `{registry}` unpacks to {got}, but its signed line says {}",
                    line.name, line.version, line.checksum
                ),
            ));
        }
        Ok(entry)
    }
}

/// #2330 (C): a `withdrawn` reason as the client prints it. It is untrusted
/// text from the index, so control characters are removed (it can't move the
/// cursor, clear the line or forge another diagnostic), it is cut to
/// [`protocol::WITHDRAWN_MAX`] characters, and it is quoted.
pub(crate) fn sanitise_reason(reason: &str) -> String {
    let clean: String = reason.chars().filter(|c| !c.is_control()).collect();
    let mut cut: String = clean.chars().take(protocol::WITHDRAWN_MAX).collect();
    if clean.chars().count() > protocol::WITHDRAWN_MAX {
        cut.push('…');
    }
    format!("{cut:?}")
}

/// R3.1 (#2378): the account API's base, the `account` sibling of `api` —
/// or, for an `api` at the origin's root, `account` under it (resolving
/// against the root would climb out of the origin, to a host named
/// `account`).
fn account_base(api: &str) -> String {
    let api = api.trim_end_matches('/');
    let at_root = api
        .split_once("://")
        .is_some_and(|(_, rest)| !rest.contains('/'));
    if at_root {
        join(api, "account")
    } else {
        transport::resolve(api, "account")
    }
}

/// A URL's scheme, lower-cased.
fn scheme(url: &str) -> Option<String> {
    url.split_once("://").map(|(s, _)| s.to_ascii_lowercase())
}

/// The first-party registry's pinned key, once live.
fn first_party_key(name: &str) -> Option<String> {
    (name == FIRST_PARTY_REGISTRY)
        .then_some(FIRST_PARTY)
        .flatten()
        .map(|(_, key)| key.to_string())
}

/// R3.2 (#2383, E): where a publish token was found. It names the place,
/// never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenSource {
    /// `BYNK_REGISTRY_<NAME>_TOKEN`.
    Var(String),
    /// `[registries.<name>.organisations.<org>] token`.
    Organisation { registry: String, org: String },
    /// `[registries.<name>] token`.
    Registry(String),
}

impl std::fmt::Display for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenSource::Var(var) => write!(f, "`{var}`"),
            TokenSource::Organisation { registry, org } => {
                write!(f, "`[registries.{registry}.organisations.{org}] token`")
            }
            TokenSource::Registry(registry) => write!(f, "`[registries.{registry}] token`"),
        }
    }
}

/// The token for registry `name`, and where it came from:
/// `BYNK_REGISTRY_<NAME>_TOKEN`, else — R3.2 (#2383, C) — acting for `org`,
/// `[registries.<name>.organisations.<org>] token`, else
/// `[registries.<name>] token` in the user's credentials file.
fn token_for(
    name: &str,
    org: Option<&str>,
    notices: &mut Vec<String>,
) -> Option<(String, TokenSource)> {
    let var = bynk_project::manifest::registry_token_var(name);
    if let Some(t) = std::env::var(&var).ok().filter(|t| !t.trim().is_empty()) {
        return Some((t, TokenSource::Var(var)));
    }
    let path = credentials_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.permissions().mode() & 0o077 != 0
        {
            notices.push(format!(
                "`{}` is readable by others — `chmod 600` it",
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = &notices;
    let doc: toml::Table = text.parse().ok()?;
    saved_token(&doc, name, org)
}

/// The saved publish token for `name` in a credentials file: acting for
/// `org`, its own slot first, then the registry-wide one.
fn saved_token(doc: &toml::Table, name: &str, org: Option<&str>) -> Option<(String, TokenSource)> {
    let entry = doc.get("registries")?.get(name)?;
    let own = org.and_then(|org| {
        let t = entry
            .get("organisations")?
            .get(org)?
            .get("token")?
            .as_str()?;
        Some((
            t.to_owned(),
            TokenSource::Organisation {
                registry: name.to_string(),
                org: org.to_string(),
            },
        ))
    });
    own.or_else(|| {
        let t = entry.get("token")?.as_str()?;
        Some((t.to_owned(), TokenSource::Registry(name.to_string())))
    })
}

/// `$XDG_CONFIG_HOME/bynk/credentials.toml`, else
/// `~/.config/bynk/credentials.toml`.
pub(crate) fn credentials_path() -> Option<PathBuf> {
    if let Some(c) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(c).join("bynk").join("credentials.toml"));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("bynk")
            .join("credentials.toml"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_organisation_s_own_slot_is_preferred_and_only_when_acting_for_it() {
        let doc: toml::Table = r#"
            [registries.bynk]
            token = "bynk_wide"
            [registries.bynk.organisations.acme]
            token = "bynk_acme"
            [registries.solo.organisations.acme]
            token = "bynk_solo_acme"
        "#
        .parse()
        .unwrap();
        let own = TokenSource::Organisation {
            registry: "bynk".into(),
            org: "acme".into(),
        };
        let wide = TokenSource::Registry("bynk".into());
        assert_eq!(
            saved_token(&doc, "bynk", Some("acme")),
            Some(("bynk_acme".into(), own.clone()))
        );
        // Another organisation, and resolution (no organisation), fall back.
        assert_eq!(
            saved_token(&doc, "bynk", Some("acme-labs")),
            Some(("bynk_wide".into(), wide.clone()))
        );
        assert_eq!(
            saved_token(&doc, "bynk", None),
            Some(("bynk_wide".into(), wide))
        );
        // An organisation's slot never serves a client not acting for it.
        assert_eq!(saved_token(&doc, "solo", None), None);
        assert_eq!(saved_token(&doc, "solo", Some("other")), None);
        assert_eq!(saved_token(&doc, "other", Some("acme")), None);
        assert_eq!(
            own.to_string(),
            "`[registries.bynk.organisations.acme] token`"
        );
    }

    #[test]
    fn the_account_base_stays_on_the_api_origin() {
        assert_eq!(
            account_base("https://r.example/api"),
            "https://r.example/account"
        );
        assert_eq!(
            account_base("https://r.example/api/"),
            "https://r.example/account"
        );
        assert_eq!(
            account_base("https://r.example/reg/api"),
            "https://r.example/reg/account"
        );
        assert_eq!(
            account_base("https://r.example/"),
            "https://r.example/account"
        );
        assert_eq!(
            account_base("http://localhost:8730"),
            "http://localhost:8730/account"
        );
    }

    #[test]
    fn a_withdrawn_reason_is_printed_sanitised() {
        assert_eq!(sanitise_reason("a takedown"), "\"a takedown\"");
        // Control characters can't move the cursor or forge a diagnostic.
        assert_eq!(
            sanitise_reason("ok\u{1b}[2K\rerror[bynk.fake]: \"x\"\n"),
            "\"ok[2Kerror[bynk.fake]: \\\"x\\\"\""
        );
        let long = "x".repeat(protocol::WITHDRAWN_MAX + 10);
        let shown = sanitise_reason(&long);
        assert_eq!(
            shown.chars().count(),
            protocol::WITHDRAWN_MAX + 3,
            "{shown}"
        );
        assert!(shown.ends_with("…\""));
    }
}

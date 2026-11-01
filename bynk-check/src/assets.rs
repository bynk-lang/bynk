//! #1883 (static assets): the checker-side rules for `asset name = "path"`
//! declarations — the path rules, the name rule, the composition of a unit's
//! visible assets through `uses` ([DECISION G]) — and (slice 1b) the build
//! side's shared definitions: the existence check, the served URL path, and
//! the files each Worker ships.
//!
//! The checker stays disk-free (`bynk-wasm` compiles it with no filesystem):
//! the project's `assets/` folder reaches it from the caller
//! (`bynk_project::assets`), as a path set or as an
//! [`AssetFolder`](bynk_project::assets::AssetFolder) whose reader it calls
//! for the declared files alone ([`read_declared_files`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bynk_syntax::ast::{AssetDecl, CommonsItem, FnName, Ident};
use bynk_syntax::error::CompileError;

use crate::project_model::{ErrorSink, UnitInfo};
use bynk_project::ParsedFile;

/// Why `path` is not a safe asset path, or `None` when it is.
///
/// An asset is public by construction, and the path is joined onto the
/// project root to find the file, so a path that can name something outside
/// the project is rejected outright: an absolute path (`/etc/x`, `\x`,
/// `C:/x`), and any `..` segment. Both separators count, so a Windows-style
/// path is judged the same on every host. An empty path names no file.
pub fn unsafe_asset_path_reason(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return Some("the path is empty");
    }
    let bytes = path.as_bytes();
    let has_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if path.starts_with('/') || path.starts_with('\\') || has_drive {
        return Some("the path is absolute");
    }
    if path.split(['/', '\\']).any(|seg| seg == "..") {
        return Some("the path contains a `..` segment");
    }
    None
}

/// `bynk.asset.unsafe_path` for `a`, when [`unsafe_asset_path_reason`]
/// rejects its path. One constructor for the project pipeline
/// (`symbols::build_unit_table`) and the single-file compile
/// (`bynk-emit`'s `compile`), so the two report it identically.
pub fn unsafe_path_error(a: &AssetDecl) -> Option<CompileError> {
    let reason = unsafe_asset_path_reason(&a.path)?;
    Some(
        CompileError::new(
            "bynk.asset.unsafe_path",
            a.path_span,
            format!("asset `{}` has an unsafe path: {reason}", a.name.name),
        )
        .with_note(
            "an asset path is relative to the project root and must stay inside it; assets are public, so nothing outside the project may be named",
        ),
    )
}

/// `bynk.asset.outside_assets_dir` for `a`, when its path is safe but does
/// not lie inside the project's assets folder
/// ([`bynk_project::assets::PROJECT_ASSETS_DIR`]). Assets are public, so they
/// come only from that folder, and a declaration can never publish `.env` or
/// a source file by mistake (#1883, slice 1b).
pub fn outside_assets_dir_error(a: &AssetDecl) -> Option<CompileError> {
    if unsafe_asset_path_reason(&a.path).is_some()
        || bynk_project::assets::is_in_assets_dir(&a.path)
    {
        return None;
    }
    let dir = bynk_project::assets::PROJECT_ASSETS_DIR;
    Some(
        CompileError::new(
            "bynk.asset.outside_assets_dir",
            a.path_span,
            format!(
                "asset `{}` must name a file in the project's `{dir}/` folder",
                a.name.name
            ),
        )
        .with_note(format!(
            "assets are public, so they come only from `{dir}/`; move the file there and name it `{dir}/…`"
        )),
    )
}

/// The path rules for `a`, in order: [`unsafe_path_error`], then
/// [`outside_assets_dir_error`] (at most one of the two applies).
pub fn path_errors(a: &AssetDecl) -> Option<CompileError> {
    unsafe_path_error(a).or_else(|| outside_assets_dir_error(a))
}

/// The URL path prefix every asset is served under. A handler route under it
/// is reserved for assets (slice 2's check), and `_headers` gives everything
/// under it an immutable cache lifetime.
pub const SERVED_PREFIX: &str = "/_assets/";

/// `bynk.asset.not_served` (a warning) for each asset among `items`, for a
/// deployable Bundle build: a Bundle ships no Worker and so no files, yet each
/// module still carries its asset's `.url`, which would 404 wherever the
/// output is served. A warning, not an error, because a Bundle build is also
/// how `bynkc test` compiles; the caller raises it only for a build without
/// tests (review of #1904). A helper here keeps `bynk-emit`'s `project.rs`
/// free of syntax-tree types (the `ast_importers` probe).
pub fn bundle_not_served_warnings(items: &[CommonsItem]) -> Vec<CompileError> {
    items
        .iter()
        .filter_map(|item| match item {
            CommonsItem::Asset(a) => Some(
                CompileError::new(
                    "bynk.asset.not_served",
                    a.span,
                    format!(
                        "asset `{}` is not served by a Bundle build: its `.url` names a file this build does not ship",
                        a.name.name
                    ),
                )
                .with_note(
                    "assets are served beside a Worker; build with `--target workers` (as `bynk dev` and `bynk deploy` do)",
                ),
            ),
            _ => None,
        })
        .collect()
}

/// The URL path the platform serves the asset file `source` (a normalised
/// `assets/…` key) with content `bytes` at:
/// `/_assets/<stem>.<hash>.<ext>`, where `<hash>` is the content's FNV-1a 64
/// ([`crate::contract::fnv1a64_hex`]) and `<stem>`/`<ext>` are the file
/// name's, split at its last `.` (no `.<ext>` when it has none).
///
/// Only the file name is kept, not its folders: the hash already tells two
/// files apart, and the same bytes under the same name are one served file,
/// however many declarations name them. Any character outside
/// `[A-Za-z0-9._-]` becomes `-`, so `.url` never needs percent-encoding.
/// Reproducible: it depends only on the name and the bytes.
pub fn served_path(source: &str, bytes: &[u8]) -> String {
    served_path_with_hash(source, &crate::contract::fnv1a64_hex(bytes))
}

/// [`served_path`] with the content hash supplied rather than computed — the
/// one definition of the served name's shape, shared with the editor, which
/// lists the `assets/` folder but never reads its bytes and so shows
/// `served_path_with_hash(source, "<hash>")` (#1883, editor tooling).
pub fn served_path_with_hash(source: &str, hash: &str) -> String {
    let name = source.rsplit('/').next().unwrap_or(source);
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    match clean.rfind('.') {
        Some(dot) if dot > 0 => {
            format!(
                "{SERVED_PREFIX}{}.{hash}.{}",
                &clean[..dot],
                &clean[dot + 1..]
            )
        }
        _ => format!("{SERVED_PREFIX}{clean}.{hash}"),
    }
}

/// The `Content-Type` the platform serves `served` (a URL path) with, by its
/// extension — the table measured against `wrangler dev` 4.107.0 (#1883,
/// slice 3), so a suite fetching an asset (`svc.GET(style)`) sees what
/// production sends. Text types carry `; charset=utf-8`. A file with no
/// extension is served with no content type, here `""`; an extension the
/// table does not know is `application/octet-stream`.
pub fn content_type_for(served: &str) -> &'static str {
    let name = served.rsplit('/').next().unwrap_or(served);
    let Some((_, ext)) = name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()) else {
        return "";
    };
    match ext.to_ascii_lowercase().as_str() {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/vnd.microsoft.icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The key a declaration's served path is filed under
/// ([`declared_served_paths`]): its normalised source path, plus its fixed
/// URL path when it has one — so one file may be served both content-hashed
/// and at a fixed path.
pub fn asset_key(a: &AssetDecl) -> String {
    let source = bynk_project::assets::normalise_asset_path(&a.path);
    match &a.at {
        Some((at, _)) => format!("{source} at {at}"),
        None => source,
    }
}

/// Every declared asset's served path, keyed by [`asset_key`] — what the
/// emitter writes as each `.url`: its fixed path (`at "/favicon.ico"`,
/// slice 2) or its content-hashed one ([`served_path`]). Only declarations
/// whose file is in `files` appear (a missing one is
/// `bynk.asset.missing_file`).
pub fn declared_served_paths(
    parsed: &[ParsedFile],
    files: &BTreeMap<String, Vec<u8>>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for pf in parsed {
        for item in pf.items() {
            let CommonsItem::Asset(a) = item else {
                continue;
            };
            let source = bynk_project::assets::normalise_asset_path(&a.path);
            if let Some(bytes) = files.get(&source) {
                let served = match &a.at {
                    Some((at, _)) => at.clone(),
                    None => served_path(&source, bytes),
                };
                out.insert(asset_key(a), served);
            }
        }
    }
    out
}

/// The units reachable from `start` over `edges` (transitively), `start`
/// included, in sorted order.
fn closure<'a>(start: &'a str, edges: &[&'a HashMap<String, Vec<String>>]) -> BTreeSet<&'a str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = vec![start];
    while let Some(unit) = stack.pop() {
        if !seen.insert(unit) {
            continue;
        }
        for map in edges {
            for next in map.get(unit).into_iter().flatten() {
                stack.push(next);
            }
        }
    }
    seen
}

/// The files a context's Worker ships, keyed by served path, each mapped to
/// its normalised source path. Sorted and deduplicated by served path, so the
/// output is reproducible.
///
/// - **Content-hashed assets** come from every unit the context reaches over
///   `uses` **and** `consumes`, transitively. Through `uses`, a commons may
///   return another commons' `.url`; through `consumes`, another context may
///   return one of its own in a fragment this Worker serves (#1883, slice 2).
///   Either way the URL resolves against this Worker, so it ships the file.
///   Hashed names never collide, so the duplication costs only bytes.
/// - **Fixed-path assets** (`at "/favicon.ico"`) come over `uses` only: they
///   belong to the root of the Worker that declares them, and shipping a
///   consumed context's fixed paths would collide with this one's own.
pub fn deployed_assets(
    context: &str,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_tables: &HashMap<String, crate::symbols::UnitTable>,
    served: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let hashed_units = closure(context, &[unit_uses, unit_consumes]);
    let fixed_units = closure(context, &[unit_uses]);
    let mut out = BTreeMap::new();
    for unit in &hashed_units {
        let Some(table) = unit_tables.get(*unit) else {
            continue;
        };
        for decl in table.assets.values() {
            if decl.at.is_some() && !fixed_units.contains(unit) {
                continue;
            }
            if let Some(path) = served.get(&asset_key(decl)) {
                out.insert(
                    path.clone(),
                    bynk_project::assets::normalise_asset_path(&decl.path),
                );
            }
        }
    }
    out
}

/// Why `at` is not a valid fixed URL path for an asset, or `None` when it is.
/// It must be an absolute URL path naming a file: starting with `/`, not
/// ending in `/`, with no empty, `.` or `..` segment, and only
/// `[A-Za-z0-9._~-]` between the slashes (so it needs no encoding). It may not
/// lie under [`SERVED_PREFIX`] (the content-hashed files' space) or `/_bynk/`
/// (Bynk's own dispatch), nor be one of the root files the platform reads as
/// configuration (`/_headers`, `/_redirects`, `/.assetsignore`).
pub fn at_path_reason(at: &str) -> Option<&'static str> {
    let Some(rest) = at.strip_prefix('/') else {
        return Some("it does not start with `/`");
    };
    if rest.is_empty() || rest.ends_with('/') {
        return Some("it does not name a file (it ends with `/`)");
    }
    if rest
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Some("it has an empty, `.` or `..` segment");
    }
    if !rest
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '~' | '-'))
    {
        return Some("it has a character outside `[A-Za-z0-9._~-]`");
    }
    let first = rest.split('/').next().unwrap_or("");
    if first == SERVED_PREFIX.trim_matches('/') {
        return Some("it is under `/_assets/`, which content-hashed assets use");
    }
    if first == "_bynk" {
        return Some("it is under `/_bynk/`, which Bynk's own dispatch uses");
    }
    // Review of #1906: at the root of a Worker's assets directory these names
    // are configuration, not files — the platform reads `_headers` and
    // `_redirects`, wrangler reads `.assetsignore` — and the compiler writes
    // its own `_headers` there.
    if matches!(rest, "_headers" | "_redirects" | ".assetsignore") {
        return Some("the platform reads that file as configuration, not as an asset");
    }
    None
}

/// `bynk.asset.invalid_at_path` for `a`, when its `at` path fails
/// [`at_path_reason`].
pub fn at_path_error(a: &AssetDecl) -> Option<CompileError> {
    let (at, span) = a.at.as_ref()?;
    let reason = at_path_reason(at)?;
    Some(
        CompileError::new(
            "bynk.asset.invalid_at_path",
            *span,
            format!("asset `{}` cannot be served at `{at}`: {reason}", a.name.name),
        )
        .with_note(
            "a fixed asset path is a URL path naming one file, such as `/favicon.ico` or `/robots.txt`",
        ),
    )
}

/// Whether the HTTP route `pattern` (`/a/:id/b`) matches the URL path `path`:
/// the same number of segments, each literal equal or a `:param`. Routes have
/// no wildcards, so this is the whole matching rule.
fn route_matches(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let u: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    p.len() == u.len()
        && p.iter()
            .zip(&u)
            .all(|(ps, us)| ps.starts_with(':') || ps == us)
}

/// #1883, slice 2 ([DECISION C]): fixed asset paths against each Worker's
/// routes. For every context, over the assets its `uses` closure declares
/// with an `at` path:
///
/// - two different files at one fixed path is `bynk.asset.path_conflict`;
/// - a fixed path equal to one of the context's literal HTTP routes is
///   `bynk.asset.shadows_route`: the platform answers that path before the
///   Worker runs, for **every** method (a `POST` gets the platform's `405`,
///   checked against `wrangler dev` 4.107.0), so the handler is unreachable;
/// - a fixed path a parameterised route matches (`/:name`) is the warning
///   `bynk.asset.may_shadow_route`: the route still serves every other path.
///
/// Content-hashed paths need no check here: handlers may not use the
/// `/_assets/` prefix at all (`bynk.http.reserved_prefix`).
pub fn phase_asset_routes(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, bynk_project::UnitKind>,
    unit_uses: &HashMap<String, Vec<String>>,
    errors: &mut ErrorSink,
) {
    use bynk_syntax::ast::HandlerKind;
    // A commons used by two contexts would otherwise report its conflict
    // twice; report each (file, span, code) once.
    let mut reported: BTreeSet<(std::path::PathBuf, usize, &'static str)> = BTreeSet::new();
    let mut push = |file: std::path::PathBuf, err: CompileError, errors: &mut ErrorSink| {
        if reported.insert((file.clone(), err.span.start, err.category)) {
            errors.push_for(Some(&file), err);
        }
    };
    for (ctx, kind) in kinds {
        if *kind != bynk_project::UnitKind::Context {
            continue;
        }
        // Fixed paths this Worker serves: at -> source file.
        let mut fixed: BTreeMap<String, String> = BTreeMap::new();
        for unit in closure(ctx, &[unit_uses]) {
            for &i in groups.get(unit).into_iter().flatten() {
                for item in parsed[i].items() {
                    let CommonsItem::Asset(a) = item else {
                        continue;
                    };
                    let Some((at, span)) = &a.at else { continue };
                    if path_errors(a).is_some() || at_path_reason(at).is_some() {
                        continue;
                    }
                    let source = bynk_project::assets::normalise_asset_path(&a.path);
                    // Review of #1906: a path that is a folder of another
                    // (`/logo` and `/logo/icon.png`) cannot be written, since
                    // one name would be both a file and a directory.
                    let nested = fixed.keys().find(|other| {
                        other.starts_with(&format!("{at}/")) || at.starts_with(&format!("{other}/"))
                    });
                    if let Some(other) = nested {
                        push(
                            parsed[i].identity_path(),
                            CompileError::new(
                                "bynk.asset.path_conflict",
                                *span,
                                format!(
                                    "the Worker for `{ctx}` cannot serve files at both `{other}` and `{at}`: one would be a folder of the other"
                                ),
                            )
                            .with_note("a fixed asset path names one file, so it cannot also be a folder; give one of them another path"),
                            errors,
                        );
                        continue;
                    }
                    match fixed.get(at) {
                        Some(prev) if *prev != source => push(
                            parsed[i].identity_path(),
                            CompileError::new(
                                "bynk.asset.path_conflict",
                                *span,
                                format!(
                                    "the Worker for `{ctx}` would serve two files at `{at}`: `{prev}` and `{source}`"
                                ),
                            )
                            .with_note("each fixed asset path names one file; give one of them another path"),
                            errors,
                        ),
                        Some(_) => {}
                        None => {
                            fixed.insert(at.clone(), source);
                        }
                    }
                }
            }
        }
        if fixed.is_empty() {
            continue;
        }
        for &i in groups.get(ctx.as_str()).into_iter().flatten() {
            for item in parsed[i].items() {
                let CommonsItem::Service(svc) = item else {
                    continue;
                };
                for h in &svc.handlers {
                    let HandlerKind::Http { method, path } = &h.kind else {
                        continue;
                    };
                    for (at, source) in &fixed {
                        if !route_matches(path, at) {
                            continue;
                        }
                        let literal = !path.split('/').any(|s| s.starts_with(':'));
                        let err = if literal {
                            CompileError::new(
                                "bynk.asset.shadows_route",
                                h.span,
                                format!(
                                    "`{} {path}` is unreachable: the asset `{source}` is served at `{at}`",
                                    method.as_str()
                                ),
                            )
                            .with_note(
                                "the platform answers an asset's path before the Worker runs, for every method; move the asset or the route",
                            )
                        } else {
                            CompileError::new(
                                "bynk.asset.may_shadow_route",
                                h.span,
                                format!(
                                    "`{} {path}` will not see requests for `{at}`: the asset `{source}` is served there",
                                    method.as_str()
                                ),
                            )
                            .with_note(
                                "the platform answers an asset's path before the Worker runs; every other path still reaches this route",
                            )
                        };
                        push(parsed[i].identity_path(), err, errors);
                    }
                }
            }
        }
    }
}

/// `bynk.asset.missing_file` for each asset whose (otherwise valid) path names
/// no file in `available` — the project's `assets/` folder as the caller read
/// it (`bynk_project::assets`). `None` skips the check: nothing on disk was
/// consulted (an in-memory check). A build passes `Some`, an empty set when it
/// had no filesystem, so it never invents a URL for a file nobody read.
///
/// A path that already fails [`path_errors`] is not also reported missing.
pub fn phase_asset_files(
    parsed: &[ParsedFile],
    available: Option<&BTreeSet<String>>,
    errors: &mut ErrorSink,
) {
    let Some(available) = available else { return };
    for pf in parsed {
        for item in pf.items() {
            let CommonsItem::Asset(a) = item else {
                continue;
            };
            if path_errors(a).is_some() {
                continue;
            }
            let key = bynk_project::assets::normalise_asset_path(&a.path);
            if !available.contains(&key) {
                errors.push_for(
                    Some(&pf.identity_path()),
                    CompileError::new(
                        "bynk.asset.missing_file",
                        a.path_span,
                        format!("asset `{}` names `{key}`, which does not exist", a.name.name),
                    )
                    .with_note(
                        "an asset must be a regular file in the project's `assets/` folder; symbolic links are not followed",
                    ),
                );
            }
        }
    }
}

/// The bytes of every declared asset file in `folder`, keyed by normalised
/// path — read through the folder's reader once each, and only for a file a
/// declaration names, so a large file nothing declares is never read. A
/// declaration whose path fails [`path_errors`] or names no listed file is
/// skipped ([`phase_asset_files`] reports the latter); a listed file the
/// reader fails on is `bynk.asset.unreadable_file`, and is left out.
pub fn read_declared_files(
    parsed: &[ParsedFile],
    folder: &bynk_project::assets::AssetFolder,
    errors: &mut ErrorSink,
) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut failed: BTreeSet<String> = BTreeSet::new();
    for pf in parsed {
        for item in pf.items() {
            let CommonsItem::Asset(a) = item else {
                continue;
            };
            if path_errors(a).is_some() {
                continue;
            }
            let key = bynk_project::assets::normalise_asset_path(&a.path);
            if !folder.files().contains(&key) || out.contains_key(&key) {
                continue;
            }
            if failed.contains(&key) {
                // Reported once per declaration, read once per file.
                errors.push_for(Some(&pf.identity_path()), unreadable_file_error(a, &key));
                continue;
            }
            match folder.read(&key) {
                Ok(bytes) => {
                    out.insert(key, bytes);
                }
                Err(e) => {
                    errors.push_for(
                        Some(&pf.identity_path()),
                        unreadable_file_error(a, &key).with_note(format!("the read failed: {e}")),
                    );
                    failed.insert(key);
                }
            }
        }
    }
    out
}

fn unreadable_file_error(a: &AssetDecl, key: &str) -> CompileError {
    CompileError::new(
        "bynk.asset.unreadable_file",
        a.path_span,
        format!(
            "asset `{}` names `{key}`, which could not be read",
            a.name.name
        ),
    )
}

/// `bynk.asset.needs_project` for `a`: a single-file build emits one module
/// and no Worker, and has no project root to find `assets/` under, so it
/// cannot ship a file. Checking a single file (`bynk check page.bynk`) is
/// unaffected; building one that declares an asset is refused. Build the
/// project instead.
pub fn needs_project_error(a: &AssetDecl) -> CompileError {
    CompileError::new(
        "bynk.asset.needs_project",
        a.span,
        format!(
            "asset `{}` cannot be built from a single file",
            a.name.name
        ),
    )
    .with_note(
        "an asset is served beside a Worker, so it needs a project build (`bynk dev`, `bynkc compile <project dir>`) with the file in the project's `assets/` folder",
    )
}

/// The name rule for asset `a`, given what its unit already declares under
/// that name: an earlier asset is `bynk.asset.duplicate`; a free function or a
/// type is `bynk.resolve.name_conflict` (an asset is a value, in the
/// function namespace, and a type name in value position is an error). `None`
/// when the name is free. One rule for the project pipeline
/// (`symbols::build_unit_table`) and the single-file compile, which has no
/// unit table (review of #1903).
pub fn name_error(
    a: &AssetDecl,
    prev_asset: Option<&Ident>,
    prev_fn: Option<&Ident>,
    prev_type: Option<&Ident>,
) -> Option<CompileError> {
    let name = &a.name.name;
    if let Some(prev) = prev_asset {
        Some(
            CompileError::new(
                "bynk.asset.duplicate",
                a.name.span,
                format!("asset `{name}` is already declared"),
            )
            .with_label(prev.span, "previously declared here"),
        )
    } else if let Some(prev) = prev_fn {
        Some(
            CompileError::new(
                "bynk.resolve.name_conflict",
                a.name.span,
                format!("asset `{name}` conflicts with a function of the same name"),
            )
            .with_label(prev.span, "function declared here"),
        )
    } else {
        prev_type.map(|prev| {
            CompileError::new(
                "bynk.resolve.name_conflict",
                a.name.span,
                format!("asset `{name}` conflicts with a type of the same name"),
            )
            .with_label(prev.span, "type declared here")
        })
    }
}

/// The single-file compile's asset diagnostics, in declaration order: each
/// asset's path check and name rule ([`name_error`], against the file's own
/// functions, types and earlier assets), and, when `build` is set, its
/// refusal ([`needs_project_error`]). Placement needs no check here: a
/// single file is always a commons (an `adapter` is a parse error).
pub fn single_file_asset_errors(items: &[CommonsItem], build: bool) -> Vec<CompileError> {
    let mut fns: HashMap<&str, &Ident> = HashMap::new();
    let mut types: HashMap<&str, &Ident> = HashMap::new();
    for item in items {
        match item {
            CommonsItem::Fn(f) => {
                if let FnName::Free(id) = &f.name {
                    fns.entry(id.name.as_str()).or_insert(id);
                }
            }
            CommonsItem::Type(t) => {
                types.entry(t.name.name.as_str()).or_insert(&t.name);
            }
            CommonsItem::Event(e) => {
                types.entry(e.name.name.as_str()).or_insert(&e.name);
            }
            _ => {}
        }
    }
    let mut assets: HashMap<&str, &Ident> = HashMap::new();
    let mut errors = Vec::new();
    for item in items {
        let CommonsItem::Asset(a) = item else {
            continue;
        };
        errors.extend(path_errors(a));
        errors.extend(at_path_error(a));
        let key = a.name.name.as_str();
        match name_error(
            a,
            assets.get(key).copied(),
            fns.get(key).copied(),
            types.get(key).copied(),
        ) {
            Some(err) => errors.push(err),
            None => {
                assets.insert(key, &a.name);
            }
        }
        if build {
            errors.push(needs_project_error(a));
        }
    }
    errors
}

/// Every asset visible in unit `name`: its own, plus those of each commons it
/// `uses` (one level, the same mixin rule `compose_unit_symbols` applies to
/// types and functions). A local asset wins over an imported one of the same
/// name; two used commons declaring the same name is
/// `bynk.uses.name_conflict` (`phase_uses_name_conflicts`), so which of them
/// is kept here is never observable in a program that checks.
pub fn compose_unit_assets(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
) -> HashMap<String, AssetDecl> {
    let Some(info) = unit_info.get(name) else {
        return HashMap::new();
    };
    let mut combined = info.table.assets.clone();
    for used in &info.uses {
        let Some(used_info) = unit_info.get(used) else {
            continue;
        };
        // #2618: a used commons' `private` assets stay in it.
        for (asset_name, decl) in used_info
            .table
            .assets
            .iter()
            .filter(|(_, d)| d.private.is_none())
        {
            // #1925: a local declaration of any kind shadows the import.
            if crate::symbols::local_value_kind(&info.table, asset_name).is_some() {
                continue;
            }
            combined
                .entry(asset_name.clone())
                .or_insert_with(|| decl.clone());
        }
    }
    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The asset files these tests' declarations name, written into each
    /// project beside its sources so the existence check passes (it reads
    /// the `assets/` folder from disk, as the editor does).
    const ASSET_FILES: &[(&str, &str)] = &[
        ("assets/a.css", "a{}"),
        ("assets/b.css", "b{}"),
        ("assets/board.css", "body{}"),
        ("assets/favicon.ico", "ico"),
        ("assets/other.ico", "other"),
    ];

    fn analyse(files: &[(&str, &str)]) -> crate::testkit::Analysed {
        let mut all = files.to_vec();
        all.extend_from_slice(ASSET_FILES);
        crate::testkit::analyse(&all)
    }

    /// #1883, slice 1b: a declared file must exist in `assets/`; a symlink
    /// there does not count.
    #[test]
    fn a_declared_file_must_exist_in_the_assets_folder() {
        let a = crate::testkit::analyse(&[(
            "c.bynk",
            "context c\n\nasset style = \"assets/missing.css\"\n",
        )]);
        a.assert_reports("bynk.asset.missing_file");

        let a = analyse(&[(
            "c.bynk",
            "context c\n\nasset style = \"./assets/board.css\"\n",
        )]);
        a.assert_clean();
    }

    /// [DECISION G]: a commons owns its stylesheet, and a context that `uses`
    /// it reads `.url` without naming the asset itself — in a free function
    /// and in a handler body, which is checked against its own per-handler
    /// view of the unit (`context_checks`).
    #[test]
    fn a_used_commons_asset_is_a_value_of_type_asset_with_a_string_url() {
        let a = analyse(&[
            (
                "compat/page.bynk",
                "commons compat.page\n\n---\nThe board's stylesheet.\n---\nasset style = \"assets/board.css\"\n\nfn link(a: Asset) -> String {\n  a.url\n}\n\nfn sheet() -> String {\n  style.url\n}\n",
            ),
            (
                "compat/board.bynk",
                "context compat.board\n\nuses compat.page\n\nfn head() -> String {\n  link(style)\n}\n\nservice web from http {\n  on GET(\"/\") () -> Effect[HttpResult[String]] by Visitor {\n    Ok(style.url)\n  }\n}\n",
            ),
        ]);
        a.assert_clean();
        assert_eq!(a.type_at("compat/page.bynk", "style.url"), "String");
        assert_eq!(a.type_at("compat/page.bynk", "a.url"), "String");
        assert_eq!(a.type_at("compat/board.bynk", "link(style)"), "String");
        assert_eq!(a.type_at("compat/board.bynk", "style.url"), "String");
    }

    /// A local binding shadows an asset of the same name, as it shadows a
    /// function; `asset` stays an ordinary identifier outside its
    /// declaration position (a contextual keyword).
    #[test]
    fn a_local_shadows_an_asset_and_asset_is_still_an_identifier() {
        let a = analyse(&[(
            "c.bynk",
            "context c\n\nasset style = \"assets/a.css\"\n\nfn f(style: Int) -> Int {\n  style + 1\n}\n\nfn g(asset: Int) -> Int {\n  asset\n}\n",
        )]);
        a.assert_clean();
        assert_eq!(a.type_at("c.bynk", "style + 1"), "Int");
    }

    /// A unit's own asset wins over a used commons' asset of the same name;
    /// only two *used* commons colliding is an error.
    #[test]
    fn a_local_asset_wins_over_a_used_one() {
        let a = analyse(&[
            (
                "lib/a.bynk",
                "commons lib.a\n\nasset style = \"assets/a.css\"\n",
            ),
            (
                "app/web.bynk",
                "context app.web\n\nuses lib.a\n\nasset style = \"assets/b.css\"\n\nfn f() -> String {\n  style.url\n}\n",
            ),
        ]);
        a.assert_clean();
    }

    /// #1930: a method on an asset's `url`, written on the chain, is a method
    /// on a `String` in a context too, not a call into a context named
    /// `style.url`. A commons never reaches the cross-context check, since it
    /// has no `consumes`.
    #[test]
    fn a_method_on_an_assets_url_is_not_a_cross_context_call() {
        let a = analyse(&[(
            "app/web.bynk",
            "context app.web\n\nasset style = \"assets/a.css\"\n\nfn hashed() -> Bool {\n  style.url.startsWith(\"/_assets/\")\n}\n",
        )]);
        a.assert_clean();
    }

    /// Review of #1937: an asset named like a consumed context's root segment
    /// does not hide that context, so a real cross-context call through it
    /// still resolves.
    #[test]
    fn an_asset_named_like_a_consumed_prefix_keeps_the_call() {
        let a = analyse(&[
            (
                "shop/payment.bynk",
                "context shop.payment\n\nservice charge {\n  on call(n: Int) -> Effect[Int] {\n    Effect.pure(n)\n  }\n}\n",
            ),
            (
                "shop/orders.bynk",
                "context shop.orders\n\nconsumes shop.payment\n\nasset shop = \"assets/a.css\"\n\nservice place {\n  on call() -> Effect[Int] {\n    let n <- shop.payment.charge(1)\n    Effect.pure(n)\n  }\n}\n",
            ),
        ]);
        a.assert_clean();
    }

    #[test]
    fn two_used_commons_declaring_one_asset_name_conflict() {
        let a = analyse(&[
            (
                "lib/a.bynk",
                "commons lib.a\n\nasset style = \"assets/a.css\"\n",
            ),
            (
                "lib/b.bynk",
                "commons lib.b\n\nasset style = \"assets/b.css\"\n",
            ),
            (
                "app/web.bynk",
                "context app.web\n\nuses lib.a\nuses lib.b\n\nfn f() -> String {\n  style.url\n}\n",
            ),
        ]);
        a.assert_reports("bynk.uses.name_conflict");
    }

    /// A unit split across files has one asset namespace: a second
    /// declaration in a sibling file is a duplicate, reported once, and a
    /// sibling's asset is visible without any `uses`.
    #[test]
    fn a_multi_file_unit_shares_one_asset_namespace() {
        let dup = analyse(&[
            ("c/a.bynk", "context c\n\nasset style = \"assets/a.css\"\n"),
            ("c/b.bynk", "context c\n\nasset style = \"assets/b.css\"\n"),
        ]);
        assert_eq!(
            dup.categories()
                .iter()
                .filter(|c| **c == "bynk.asset.duplicate")
                .count(),
            1,
            "{}",
            dup.render()
        );
        let shared = analyse(&[
            ("c/a.bynk", "context c\n\nasset style = \"assets/a.css\"\n"),
            (
                "c/b.bynk",
                "context c\n\nfn f() -> String {\n  style.url\n}\n",
            ),
        ]);
        shared.assert_clean();
    }

    /// Review of #1903: a local asset must not hide a real conflict between
    /// two used commons over a *type* (or function) name. An asset only wins
    /// in value position; a type position still resolves through the
    /// imported types, so the ambiguity remains and must be reported.
    #[test]
    fn a_local_asset_does_not_hide_a_two_import_type_conflict() {
        let a = analyse(&[
            ("lib/a.bynk", "commons lib.a\n\ntype Style = Int\n"),
            ("lib/b.bynk", "commons lib.b\n\ntype Style = String\n"),
            (
                "app/web.bynk",
                "context app.web\n\nuses lib.a\nuses lib.b\n\nasset Style = \"assets/a.css\"\n\ntype Page = { s: Style }\n",
            ),
        ]);
        a.assert_reports("bynk.uses.name_conflict");
    }

    #[test]
    fn an_asset_has_only_a_url_field() {
        let a = analyse(&[(
            "c.bynk",
            "context c\n\nasset style = \"assets/a.css\"\n\nfn f() -> String {\n  style.path\n}\n",
        )]);
        a.assert_reports("bynk.types.unknown_field");
    }

    /// Placement, duplicates, name conflicts, path safety, the boundary rule
    /// and the reserved type name, one project each.
    #[test]
    fn each_declaration_rule_reports_its_own_code() {
        for (src, code) in [
            (
                "adapter c\n\nbinding \"./c.js\"\n\nasset style = \"assets/a.css\"\n",
                "bynk.asset.outside_context_or_commons",
            ),
            (
                "context c\n\nasset style = \"assets/a.css\"\nasset style = \"assets/b.css\"\n",
                "bynk.asset.duplicate",
            ),
            (
                "context c\n\nfn style() -> Int {\n  1\n}\n\nasset style = \"assets/a.css\"\n",
                "bynk.resolve.name_conflict",
            ),
            (
                "context c\n\ntype Style = Int\n\nasset Style = \"assets/a.css\"\n",
                "bynk.resolve.name_conflict",
            ),
            (
                "context c\n\nasset style = \"/etc/passwd\"\n",
                "bynk.asset.unsafe_path",
            ),
            (
                "context c\n\nasset style = \"../x.css\"\n",
                "bynk.asset.unsafe_path",
            ),
            (
                "context c\n\ntype Page = { style: Asset }\n",
                "bynk.types.asset_at_boundary",
            ),
            (
                "context c\n\ntype Asset = Int\n",
                "bynk.resolve.reserved_builtin_type",
            ),
            (
                "context c\n\nasset env = \".env\"\n",
                "bynk.asset.outside_assets_dir",
            ),
            (
                "context c\n\nasset page = \"src/c.bynk\"\n",
                "bynk.asset.outside_assets_dir",
            ),
            (
                "context c\n\nagent Board {\n  key id: String\n\n  store style: Cell[Asset]\n\n  on call get() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n",
                "bynk.types.asset_at_boundary",
            ),
        ] {
            let a = analyse(&[("c.bynk", src)]);
            a.assert_reports(code);
        }
    }

    #[test]
    fn served_path_is_hashed_flat_and_url_safe() {
        let h = crate::contract::fnv1a64_hex(b"body{}");
        assert_eq!(
            served_path("assets/board.css", b"body{}"),
            format!("/_assets/board.{h}.css")
        );
        assert_eq!(
            served_path("assets/css/board.min.css", b"body{}"),
            format!("/_assets/board.min.{h}.css")
        );
        assert_eq!(
            served_path("assets/LICENSE", b"body{}"),
            format!("/_assets/LICENSE.{h}")
        );
        assert_eq!(
            served_path("assets/.nojekyll", b"body{}"),
            format!("/_assets/.nojekyll.{h}")
        );
        assert_eq!(
            served_path("assets/my file é.css", b"body{}"),
            format!("/_assets/my-file--.{h}.css")
        );
        assert_ne!(
            served_path("assets/board.css", b"a"),
            served_path("assets/board.css", b"b"),
            "a content change changes the URL"
        );
    }

    /// A Worker ships the files of its whole `uses` closure, not just the
    /// first level: `ctx -> a -> b -> c`, and only `c` declares an asset.
    #[test]
    fn deployed_assets_follow_the_transitive_uses_closure() {
        fn decl(name: &str, path: &str) -> AssetDecl {
            AssetDecl {
                name: Ident {
                    name: name.to_string(),
                    span: Default::default(),
                },
                path: path.to_string(),
                path_span: Default::default(),
                at: None,
                documentation: None,
                span: Default::default(),
                trivia: Default::default(),
                private: None,
            }
        }
        let mut tables: HashMap<String, crate::symbols::UnitTable> = HashMap::new();
        for unit in ["ctx", "a", "b", "c", "unused"] {
            tables.insert(unit.to_string(), Default::default());
        }
        tables
            .get_mut("c")
            .unwrap()
            .assets
            .insert("style".into(), decl("style", "./assets/board.css"));
        tables
            .get_mut("unused")
            .unwrap()
            .assets
            .insert("logo".into(), decl("logo", "assets/logo.png"));
        let uses: HashMap<String, Vec<String>> =
            [("ctx", vec!["a"]), ("a", vec!["b"]), ("b", vec!["c", "a"])]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
                .collect();
        let served: BTreeMap<String, String> = [
            (
                "assets/board.css".to_string(),
                "/_assets/board.1.css".to_string(),
            ),
            (
                "assets/logo.png".to_string(),
                "/_assets/logo.2.png".to_string(),
            ),
        ]
        .into_iter()
        .collect();
        let deployed = deployed_assets("ctx", &uses, &HashMap::new(), &tables, &served);
        assert_eq!(
            deployed.into_iter().collect::<Vec<_>>(),
            vec![(
                "/_assets/board.1.css".to_string(),
                "assets/board.css".to_string()
            )]
        );
    }

    #[test]
    fn at_path_reason_accepts_a_file_path_and_rejects_the_rest() {
        for ok in [
            "/favicon.ico",
            "/robots.txt",
            "/.well-known/security.txt",
            "/a/b-c_d~e.f",
        ] {
            assert_eq!(at_path_reason(ok), None, "{ok} is valid");
        }
        for bad in [
            "favicon.ico",
            "/",
            "/dir/",
            "//x",
            "/a/../b",
            "/./a",
            "/a b.txt",
            "/a?b",
            "/_assets/x.css",
            "/_assets",
            "/_bynk/x",
            // Review of #1906: files the platform reads as configuration.
            "/_headers",
            "/_redirects",
            "/.assetsignore",
        ] {
            assert!(at_path_reason(bad).is_some(), "{bad} is rejected");
        }
    }

    #[test]
    fn route_matching_is_segment_wise_with_params() {
        assert!(route_matches("/favicon.ico", "/favicon.ico"));
        assert!(route_matches("/:page", "/favicon.ico"));
        assert!(route_matches("/a/:id", "/a/b"));
        assert!(!route_matches("/a/:id", "/a"));
        assert!(!route_matches("/:page", "/a/b"));
        assert!(!route_matches("/favicon.png", "/favicon.ico"));
    }

    const HANDLER: &str = "service web from http {\n  on GET(\"ROUTE\") () -> Effect[HttpResult[String]] by Visitor {\n    Ok(\"x\")\n  }\n}\n";

    fn context_with(asset: &str, route: &str) -> String {
        format!(
            "context c\n\n{asset}\n\n{}",
            HANDLER.replace("ROUTE", route)
        )
    }

    /// #1883, slice 2: a fixed path against the Worker's own routes — an
    /// exact literal route is unreachable (an error, for every method); a
    /// parameterised one only loses that path (a warning).
    #[test]
    fn a_fixed_path_is_checked_against_the_workers_routes() {
        let fav = "asset favicon = \"assets/favicon.ico\" at \"/favicon.ico\"";
        analyse(&[("c.bynk", &context_with(fav, "/favicon.ico"))])
            .assert_reports("bynk.asset.shadows_route");
        let param = context_with(fav, "/:page").replace("() ->", "(page: String) ->");
        let a = analyse(&[("c.bynk", &param)]);
        a.assert_reports("bynk.asset.may_shadow_route");
        assert!(
            a.error_categories().is_empty(),
            "a warning only: {}",
            a.render()
        );
        analyse(&[("c.bynk", &context_with(fav, "/home"))]).assert_clean();
        // Content-hashed assets never meet a route: `/_assets/` is reserved.
        analyse(&[(
            "c.bynk",
            &context_with("asset style = \"assets/board.css\"", "/_assets/x.css"),
        )])
        .assert_reports("bynk.http.reserved_prefix");
    }

    /// Two different files at one fixed path in one Worker conflict, whether
    /// declared together or reached through `uses`; the same file twice is
    /// fine. And an invalid `at` path is reported at the declaration.
    #[test]
    fn fixed_paths_conflict_and_validate() {
        let a = analyse(&[
            (
                "lib/a.bynk",
                "commons lib.a\n\nasset icon = \"assets/favicon.ico\" at \"/favicon.ico\"\n",
            ),
            (
                "app/web.bynk",
                "context app.web\n\nuses lib.a\n\nasset other = \"assets/other.ico\" at \"/favicon.ico\"\n",
            ),
        ]);
        a.assert_reports("bynk.asset.path_conflict");
        analyse(&[(
            "c.bynk",
            "context c\n\nasset a = \"assets/favicon.ico\" at \"/favicon.ico\"\nasset b = \"assets/favicon.ico\" at \"/favicon.ico\"\n",
        )])
        .assert_clean();
        analyse(&[(
            "c.bynk",
            "context c\n\nasset a = \"assets/favicon.ico\" at \"/_assets/favicon.ico\"\n",
        )])
        .assert_reports("bynk.asset.invalid_at_path");
        // Review of #1906: one path cannot be both a file and a folder.
        analyse(&[(
            "c.bynk",
            "context c\n\nasset logo = \"assets/favicon.ico\" at \"/logo\"\nasset icon = \"assets/other.ico\" at \"/logo/icon.ico\"\n",
        )])
        .assert_reports("bynk.asset.path_conflict");
    }

    /// Hashed assets ship over `uses` and `consumes`; fixed ones over `uses`
    /// only, so a consumed context's root files stay its own.
    #[test]
    fn consumes_ships_hashed_assets_but_not_fixed_ones() {
        fn decl(name: &str, path: &str, at: Option<&str>) -> AssetDecl {
            AssetDecl {
                name: Ident {
                    name: name.to_string(),
                    span: Default::default(),
                },
                path: path.to_string(),
                path_span: Default::default(),
                at: at.map(|a| (a.to_string(), Default::default())),
                documentation: None,
                span: Default::default(),
                trivia: Default::default(),
                private: None,
            }
        }
        let mut tables: HashMap<String, crate::symbols::UnitTable> = HashMap::new();
        for unit in ["a", "b"] {
            tables.insert(unit.to_string(), Default::default());
        }
        let b = tables.get_mut("b").unwrap();
        b.assets
            .insert("badge".into(), decl("badge", "assets/badge.svg", None));
        b.assets.insert(
            "robots".into(),
            decl("robots", "assets/robots.txt", Some("/robots.txt")),
        );
        let consumes: HashMap<String, Vec<String>> = [("a".to_string(), vec!["b".to_string()])]
            .into_iter()
            .collect();
        let served: BTreeMap<String, String> = [
            (
                "assets/badge.svg".to_string(),
                "/_assets/badge.1.svg".to_string(),
            ),
            (
                "assets/robots.txt at /robots.txt".to_string(),
                "/robots.txt".to_string(),
            ),
        ]
        .into_iter()
        .collect();
        let a = deployed_assets("a", &HashMap::new(), &consumes, &tables, &served);
        assert_eq!(
            a.keys().cloned().collect::<Vec<_>>(),
            vec!["/_assets/badge.1.svg"]
        );
        let b = deployed_assets("b", &HashMap::new(), &consumes, &tables, &served);
        assert_eq!(
            b.keys().cloned().collect::<Vec<_>>(),
            vec!["/_assets/badge.1.svg", "/robots.txt"]
        );
    }

    /// #1883, slice 3: a suite names its target's assets and fetches one with
    /// `svc.GET(asset)` — `GET` only, the asset alone, and no `by` (an asset
    /// is public). Anything else that is not a route string is still a bad
    /// address.
    #[test]
    fn a_suite_fetches_an_asset_by_get_alone() {
        let web = "context t.web\n\nasset style = \"assets/board.css\"\n\nservice api from http {\n  on GET(\"/\") () -> Effect[HttpResult[String]] by Visitor {\n    Ok(style.url)\n  }\n}\n";
        let suite = |body: &str| format!("suite t.web {{\n  case \"c\" {{\n{body}\n  }}\n}}\n");
        let run = |body: &str| analyse(&[("t/web.bynk", web), ("tests/web.bynk", &suite(body))]);
        run("    expect style.url == style.url\n    let r <- api.GET(style)\n    expect r is Raw(_, _)")
            .assert_clean();
        run("    let r <- api.POST(style)\n    expect r is Raw(_, _)")
            .assert_reports("bynk.test.asset_address_method");
        run("    let r <- api.GET(style, 1)\n    expect r is Raw(_, _)")
            .assert_reports("bynk.test.service_call_arity");
        run("    let r <- api.GET(style) by Visitor\n    expect r is Raw(_, _)")
            .assert_reports("bynk.test.principal_on_asset");
        run("    let s = \"/\"\n    let r <- api.GET(s)\n    expect r is Ok(_)")
            .assert_reports("bynk.test.service_bad_address");
    }

    /// #1883, slice 3 (review of #1922): the content types measured against
    /// `wrangler dev` 4.107.0, pinned so the next wrangler bump has something
    /// to diff against, plus the edges the doc comment promises.
    #[test]
    fn content_type_for_matches_the_platform() {
        for (path, ct) in [
            ("/_assets/board.3f9c2a.css", "text/css; charset=utf-8"),
            ("/_assets/app.1.js", "text/javascript; charset=utf-8"),
            ("/_assets/app.1.mjs", "text/javascript; charset=utf-8"),
            ("/_assets/d.1.json", "application/json"),
            ("/index.html", "text/html; charset=utf-8"),
            ("/index.htm", "text/html; charset=utf-8"),
            ("/robots.txt", "text/plain; charset=utf-8"),
            ("/feed.xml", "application/xml"),
            ("/_assets/i.1.svg", "image/svg+xml"),
            ("/_assets/i.1.png", "image/png"),
            ("/_assets/i.1.jpg", "image/jpeg"),
            ("/_assets/i.1.jpeg", "image/jpeg"),
            ("/_assets/i.1.gif", "image/gif"),
            ("/_assets/i.1.webp", "image/webp"),
            ("/_assets/i.1.avif", "image/avif"),
            ("/favicon.ico", "image/vnd.microsoft.icon"),
            ("/_assets/f.1.woff", "font/woff"),
            ("/_assets/f.1.woff2", "font/woff2"),
            ("/_assets/f.1.ttf", "font/ttf"),
            ("/_assets/f.1.otf", "font/otf"),
            ("/doc.pdf", "application/pdf"),
            ("/_assets/m.1.wasm", "application/wasm"),
            ("/_assets/app.1.map", "application/json"),
            ("/data.csv", "text/csv; charset=utf-8"),
            ("/notes.md", "text/markdown; charset=utf-8"),
            ("/site.webmanifest", "application/manifest+json"),
            ("/v.mp4", "video/mp4"),
            ("/v.webm", "video/webm"),
            ("/a.mp3", "audio/mpeg"),
            ("/a.wav", "audio/wav"),
            ("/x.zip", "application/zip"),
            ("/x.bin", "application/octet-stream"),
            // Edges: an unknown extension, an uppercase one, no extension
            // (served with no content type), and a dotfile (no extension).
            ("/x.unknownext", "application/octet-stream"),
            ("/Logo.PNG", "image/png"),
            ("/_assets/README.1", "application/octet-stream"),
            ("/README", ""),
            ("/.hidden", ""),
        ] {
            assert_eq!(content_type_for(path), ct, "{path}");
        }
    }

    /// Review of #1922: an asset is fetched through an HTTP service only, and a
    /// `system` suite names only its target's assets — the files the target's
    /// Worker ships — so nothing nameable can answer `NotFound`.
    #[test]
    fn asset_fetches_are_http_only_and_target_scoped() {
        let web = "context t.web\n\nasset style = \"assets/board.css\"\n\nservice jobs from queue(\"q\") {\n  on message(m: String) -> Effect[QueueResult] {\n    Ack\n  }\n}\n\nservice api from http {\n  on GET(\"/\") () -> Effect[HttpResult[String]] by Visitor {\n    Ok(style.url)\n  }\n}\n";
        let a = analyse(&[
            ("t/web.bynk", web),
            (
                "tests/web.bynk",
                "suite t.web {\n  case \"c\" {\n    let r <- jobs.GET(style)\n    expect r is Raw(_, _)\n  }\n}\n",
            ),
        ]);
        a.assert_reports("bynk.test.service_bad_address");

        let lib = "commons t.lib\n\nasset other = \"assets/a.css\"\n";
        let a = analyse(&[
            ("t/web.bynk", web),
            ("t/lib.bynk", lib),
            (
                "tests/web_system.bynk",
                "suite t.web as system {\n  uses t.lib\n\n  case \"c\" {\n    let r <- api.GET(other)\n    expect r is Raw(_, _)\n  }\n}\n",
            ),
        ]);
        a.assert_reports("bynk.resolve.unknown_name");
    }

    /// Review of #1923: a suite names exactly what the target's Worker ships
    /// (`deployed_assets`) — so a consumed context's content-hashed asset is
    /// nameable and fetchable, at both tiers, while its fixed path (served
    /// only by its own Worker) is not.
    #[test]
    fn a_consumed_contexts_hashed_asset_is_nameable_but_its_fixed_one_is_not() {
        let other = "context t.other\n\nasset badge = \"assets/a.css\"\nasset robots = \"assets/b.css\" at \"/robots.txt\"\n\nservice fragment {\n  on call() -> Effect[String] {\n    Effect.pure(badge.url)\n  }\n}\n";
        let web = "context t.web\n\nconsumes t.other\n\nservice api from http {\n  on GET(\"/\") () -> Effect[HttpResult[String]] by Visitor {\n    let f <- t.other.fragment()\n    Ok(f)\n  }\n}\n";
        for header in ["suite t.web", "suite t.web as system"] {
            let suite = |body: &str| format!("{header} {{\n  case \"c\" {{\n{body}\n  }}\n}}\n");
            analyse(&[
                ("t/other.bynk", other),
                ("t/web.bynk", web),
                (
                    "tests/web.bynk",
                    &suite("    let r <- api.GET(badge)\n    expect r is Raw(_, _)"),
                ),
            ])
            .assert_clean();
            analyse(&[
                ("t/other.bynk", other),
                ("t/web.bynk", web),
                (
                    "tests/web.bynk",
                    &suite("    let r <- api.GET(robots)\n    expect r is Raw(_, _)"),
                ),
            ])
            .assert_reports("bynk.resolve.unknown_name");
        }
    }

    #[test]
    fn unsafe_asset_path_reason_accepts_relative_paths_and_rejects_escapes() {
        for ok in [
            "assets/board.css",
            "board.css",
            "assets/.well-known/security.txt",
            "a..b/c.css",
            "./assets/x.css",
        ] {
            assert_eq!(unsafe_asset_path_reason(ok), None, "{ok} is safe");
        }
        for (bad, why) in [
            ("", "the path is empty"),
            ("/etc/passwd", "the path is absolute"),
            ("\\x.css", "the path is absolute"),
            ("C:/x.css", "the path is absolute"),
            ("../secret.txt", "the path contains a `..` segment"),
            ("assets/../../x", "the path contains a `..` segment"),
            ("assets\\..\\x", "the path contains a `..` segment"),
            ("assets/..", "the path contains a `..` segment"),
        ] {
            assert_eq!(unsafe_asset_path_reason(bad), Some(why), "{bad}");
        }
    }
}

//! #2207 (packaging slice 1): `[package]`, `[dependencies]`,
//! `[dev-dependencies]`, `[registries]` and `[workspace]` parse and validate
//! per decisions A–G. Each refusal's exact message is pinned here, did-you-mean
//! cases included; the workspace rules that read other manifests run against
//! real directories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_project::{
    DependencySource, ProjectPathsError, check_manifest, check_manifest_str, load_workspace,
    read_package_config,
};

fn err(manifest: &str) -> String {
    check_manifest_str(manifest)
        .expect_err("the manifest is refused")
        .to_string()
}

fn ok(manifest: &str) {
    if let Err(e) = check_manifest_str(manifest) {
        panic!("{manifest:?} is refused: {e}");
    }
}

const KEY: &str = "ed25519:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

// ---------------------------------------------------------------------------
// [package] (decision D)
// ---------------------------------------------------------------------------

#[test]
fn a_full_package_is_accepted_and_read() {
    let manifest = "[package]\nname = \"acme/money\"\nversion = \"1.4.0\"\ndescription = \"Money\"\n\
                    license = \"MIT\"\nrepository = \"https://github.com/acme/money\"\n\
                    exports = [\"currency\", \"money\"]\n";
    ok(manifest);
    let root = PathBuf::from("/nonexistent-bynk-2207");
    let overlay = HashMap::from([(root.join("bynk.toml"), manifest.to_string())]);
    let package = read_package_config(&root, &overlay).expect("read");
    assert_eq!(
        (package.org.as_str(), package.package.as_str()),
        ("acme", "money")
    );
    assert_eq!(package.version.as_deref(), Some("1.4.0"));
    assert_eq!(
        package.exports,
        Some(vec!["currency".to_string(), "money".to_string()])
    );
    ok("[package]\nname = \"acme-labs/ledger_kit\"\n");
}

#[test]
fn project_and_package_are_exclusive() {
    assert_eq!(
        err("[project]\nname = \"p\"\n\n[package]\nname = \"acme/money\"\n"),
        "`bynk.toml` has both `[project]` and `[package]` — a package is named by `[package] name`, so remove `[project]`"
    );
}

#[test]
fn a_package_name_is_required_and_well_formed() {
    assert_eq!(
        err("[package]\nversion = \"1.0.0\"\n"),
        "`[package]` `name` is required: a package is named `organisation/package`, like `name = \"acme/money\"`"
    );
    assert_eq!(
        err("[package]\nname = \"money\"\n"),
        "`[package]` `name` `money` must be `organisation/package`, like `acme/money`"
    );
    assert_eq!(
        err("[package]\nname = \"acme/ledger-kit\"\n"),
        "`[package]` `name` names the package `ledger-kit`, which can't contain `-`, because it is also a name in Bynk source, where `-` is minus — did you mean `ledger_kit`?"
    );
    assert_eq!(
        err("[package]\nname = \"Acme/money\"\n"),
        "`[package]` `name` names the organisation `Acme`, which must match `[a-z][a-z0-9-]*`"
    );
    assert_eq!(
        err("[package]\nname = \"acme/context\"\n"),
        "`[package]` `name` names the package `context`, which is a Bynk keyword"
    );
    assert_eq!(
        err(&format!("[package]\nname = \"{}/money\"\n", "a".repeat(65))),
        format!(
            "`[package]` `name` names the organisation `{}`, which is longer than 64 characters",
            "a".repeat(65)
        )
    );
    assert_eq!(
        err("[package]\nname = \"bynk/money\"\n"),
        "`[package]` `name` names the organisation `bynk`, which is reserved for first-party packages"
    );
}

#[test]
fn a_package_version_is_strict_semver() {
    assert_eq!(
        err("[package]\nname = \"acme/money\"\nversion = \"1.4\"\n"),
        "`[package]` `version` `1.4` isn't a semver version, like `1.4.0`"
    );
}

#[test]
fn package_exports_are_a_list_of_unit_names() {
    assert_eq!(
        err("[package]\nname = \"acme/money\"\nexports = \"money\"\n"),
        "`[package]` `exports` must be a list of unit names, like `exports = [\"money\"]`"
    );
    assert_eq!(
        err("[package]\nname = \"acme/money\"\nexports = [\"money-x\"]\n"),
        "`[package]` `exports` lists `money-x`, which isn't a unit name"
    );
}

#[test]
fn a_package_key_suggests_the_nearest() {
    assert_eq!(
        err("[package]\nname = \"acme/money\"\nexport = []\n"),
        "`[package]` has no key named `export` — did you mean `exports`?"
    );
}

// ---------------------------------------------------------------------------
// [dependencies] / [dev-dependencies] (decisions A, G)
// ---------------------------------------------------------------------------

#[test]
fn well_formed_dependencies_pass_the_manifest_check() {
    // #2223: `[dependencies]` resolve now. Which sources a build can resolve
    // (path today; git and registry later) is the package graph's to say
    // (`bynk_project::resolve_graph`), with a coded diagnostic, not the
    // manifest check's.
    ok("[dependencies]\nfx = { package = \"globex/money\", version = \"^2\" }\n");
    ok(
        "[dependencies]\nledger_kit = { package = \"acme/ledger_kit\", git = \"https://git.acme.internal/ledger-kit\", rev = \"v0.3.0\" }\n",
    );
    ok("[dependencies.local]\npackage = \"acme/local\"\npath = \"../local\"\n");
}

#[test]
fn well_formed_dev_dependencies_are_refused_until_slice_2b() {
    assert_eq!(
        err(
            "[dev-dependencies]\nfixtures = { package = \"acme/fixtures\", path = \"../fixtures\" }\n"
        ),
        "`[dev-dependencies]` declares dependencies, but they can't be resolved until packaging slice 2b (#843) — remove them for now"
    );
    // An empty table declares nothing, so nothing is ignored.
    ok("[dependencies]\n\n[dev-dependencies]\n");
}

#[test]
fn the_dependency_reader_returns_each_source() {
    let table: toml::Table = "fx = { package = \"globex/money\", version = \"^2\" }\n\
         kit = { package = \"acme/kit\", git = \"https://x\", rev = \"v1\" }\n\
         local = { package = \"acme/local\", path = \"../local\" }\n"
        .parse()
        .unwrap();
    let deps = bynk_project::manifest::dependencies_from("dependencies", &table).unwrap();
    let sources: Vec<&DependencySource> = deps.iter().map(|d| &d.source).collect();
    assert_eq!(
        sources,
        [
            &DependencySource::Version("^2".into()),
            &DependencySource::Git {
                url: "https://x".into(),
                rev: "v1".into()
            },
            &DependencySource::Path("../local".into()),
        ]
    );
}

#[test]
fn a_dependency_is_checked_for_shape() {
    let cases = [
        (
            "fx = \"^2\"",
            "`[dependencies]` `fx` must be a table, like `{ package = \"acme/money\", version = \"^1\" }` — there is no bare-version shorthand",
        ),
        (
            "fx = { version = \"^2\" }",
            "`[dependencies]` `fx` needs `package`, the `organisation/package` it names",
        ),
        (
            "fx = { package = \"globex/money\" }",
            "`[dependencies]` `fx` must give exactly one of `version`, `git` or `path` (it gives none)",
        ),
        (
            "fx = { package = \"globex/money\", version = \"^2\", path = \"../x\" }",
            "`[dependencies]` `fx` must give exactly one of `version`, `git` or `path` (it gives `version` and `path`)",
        ),
        (
            "fx = { package = \"globex/money\", git = \"https://x\" }",
            "`[dependencies]` `fx` has a `git` source with no `rev` — pin a tag or commit, like `rev = \"v0.3.0\"`",
        ),
        (
            "fx = { package = \"globex/money\", path = \"../x\", rev = \"v1\" }",
            "`[dependencies]` `fx` has a `rev`, which only goes with `git`",
        ),
        (
            "fx = { package = \"globex/money\", version = \"*\" }",
            "`[dependencies]` `fx` `version = \"*\"` is unpinned — name a version, like `\"^1.4\"`",
        ),
        (
            "fx = { package = \"globex\", version = \"^2\" }",
            "`[dependencies]` `fx` `package` `globex` must be `organisation/package`, like `acme/money`",
        ),
        (
            "fx = { package = \"globex/fx-rates\", version = \"^2\" }",
            "`[dependencies]` `fx` `package` names the package `fx-rates`, which can't contain `-`, because it is also a name in Bynk source, where `-` is minus — did you mean `fx_rates`?",
        ),
        (
            "fx = { package = \"globex/money\", verison = \"^2\" }",
            "`[dependencies]` `fx` has no key named `verison` — did you mean `version`?",
        ),
        (
            "fx = { package = \"globex/money\", registry = \"x\" }",
            "`[dependencies]` `fx` has no key named `registry` — its keys are `package`, `version`, `git`, `rev`, `path`",
        ),
    ];
    for (entry, expected) in cases {
        assert_eq!(
            err(&format!("[dependencies]\n{entry}\n")),
            expected,
            "{entry}"
        );
    }
}

#[test]
fn an_alias_is_a_source_name() {
    assert_eq!(
        err("[dependencies]\nmatch = { package = \"acme/money\", version = \"^1\" }\n"),
        "`[dependencies]` `match` is an alias, written in source as `match.<unit>`, so it is a Bynk keyword"
    );
    assert_eq!(
        err("[dependencies]\nbynk = { package = \"acme/money\", version = \"^1\" }\n"),
        "`[dependencies]` `bynk` can't be an alias: `bynk` is reserved for the standard library"
    );
    assert_eq!(
        err(
            "[dev-dependencies]\nledger-kit = { package = \"acme/ledger_kit\", version = \"^1\" }\n"
        ),
        "`[dev-dependencies]` `ledger-kit` is an alias, written in source as `ledger-kit.<unit>`, so it can't contain `-`, because it is also a name in Bynk source, where `-` is minus — did you mean `ledger_kit`?"
    );
}

// ---------------------------------------------------------------------------
// [registries] (decisions E, G)
// ---------------------------------------------------------------------------

#[test]
fn registries_are_accepted_as_configuration() {
    ok(&format!(
        "[registries.acme-internal]\nindex = \"https://bynk.acme.internal/index\"\n\
         organisations = [\"acme\", \"acme-labs\"]\nkey = \"{KEY}\"\n\n\
         [registries.corp-mirror]\nindex = \"https://mirror.example.internal/bynk\"\nmirror-of = \"bynk\"\n\n\
         [registries.local]\nindex = \"http://localhost:8080/index\"\nmirror-of = \"acme-internal\"\n"
    ));
}

#[test]
fn a_registry_is_checked() {
    let cases = [
        (
            "[registries.bynk]\nindex = \"https://x\"\n".to_string(),
            "`[registries.bynk]` is a registry name, which is reserved: `mirror-of = \"bynk\"` already means the first-party registry",
        ),
        (
            "[registries.acme_internal]\nindex = \"https://x\"\n".to_string(),
            "`[registries.acme_internal]` is a registry name, which can't contain `_` — did you mean `acme-internal`?",
        ),
        (
            "[registries.acme]\nindex = \"https://x\"\ntoken = \"s3cret\"\n".to_string(),
            "`[registries.acme]` has a `token` — credentials belong in `BYNK_REGISTRY_ACME_TOKEN` or the user credentials file, never in `bynk.toml`",
        ),
        (
            "[registries.acme]\nindx = \"https://x\"\n".to_string(),
            "`[registries.acme]` has no key named `indx` — did you mean `index`?",
        ),
        (
            "[registries.acme]\norganisations = [\"acme\"]\n".to_string(),
            "`[registries.acme]` needs an `index` URL",
        ),
        (
            "[registries.acme]\nindex = \"http://bynk.acme.internal\"\n".to_string(),
            "`[registries.acme]` `index = \"http://bynk.acme.internal\"` must be `https://` — `http://` is allowed only for `localhost`",
        ),
        (
            "[registries.acme]\nindex = \"ftp://x\"\n".to_string(),
            "`[registries.acme]` `index = \"ftp://x\"` must be an `https://` URL",
        ),
        (
            "[registries.acme]\nindex = \"https://x\"\norganisations = [\"bynk\"]\n".to_string(),
            "`[registries.acme]` lists the organisation `bynk`, which always resolves on the first-party registry",
        ),
        (
            "[registries.acme]\nindex = \"https://x\"\nmirror-of = \"bynk\"\norganisations = [\"acme\"]\n"
                .to_string(),
            "`[registries.acme]` has both `mirror-of` and `organisations` — a mirror serves another registry's organisations and routes none of its own",
        ),
        (
            "[registries.acme]\nindex = \"https://x\"\nkey = \"ed25519:abc\"\n".to_string(),
            "`[registries.acme]` `key` must be `ed25519:` followed by the base64 of a 32-byte public key",
        ),
        (
            "[registries.a]\nindex = \"https://x\"\norganisations = [\"acme\"]\n\n\
             [registries.b]\nindex = \"https://y\"\norganisations = [\"acme\"]\n"
                .to_string(),
            "`[registries.b]` lists the organisation `acme`, which `[registries.a]` already lists — an organisation resolves on one registry",
        ),
        (
            "[registries.m]\nindex = \"https://x\"\nmirror-of = \"acme-internl\"\n\n\
             [registries.acme-internal]\nindex = \"https://y\"\n"
                .to_string(),
            "`[registries.m]` has `mirror-of = \"acme-internl\"`, which names no registry — did you mean `acme-internal`?",
        ),
        (
            "[registries.m]\nindex = \"https://x\"\nmirror-of = \"m\"\n".to_string(),
            "`[registries.m]` can't be a mirror of itself",
        ),
    ];
    for (manifest, expected) in cases {
        assert_eq!(err(&manifest), expected, "{manifest}");
    }
}

#[test]
fn a_mirror_of_a_mirror_is_refused() {
    assert_eq!(
        err(
            "[registries.a]\nindex = \"https://a\"\nmirror-of = \"bynk\"\n\n\
             [registries.b]\nindex = \"https://b\"\nmirror-of = \"a\"\n"
        ),
        "`[registries.b]` mirrors `a`, which is itself a mirror — a mirror names `bynk` or a registry that isn't one"
    );
}

// ---------------------------------------------------------------------------
// [workspace] (decision C)
// ---------------------------------------------------------------------------

#[test]
fn a_workspace_manifest_is_checked() {
    ok(
        "[workspace]\nmembers = [\"packages/*\"]\nexclude = [\"packages/scratch\"]\n\n\
        [registries.acme]\nindex = \"https://x\"\norganisations = [\"acme\"]\n",
    );
    assert_eq!(
        err("[workspace]\nexclude = []\n"),
        "`[workspace]` needs `members`, the member directories, like `members = [\"packages/*\"]`"
    );
    assert_eq!(
        err("[workspace]\nmembers = \"packages/*\"\n"),
        "`[workspace]` `members` must be a list of paths, like `members = [\"packages/*\"]`"
    );
    assert_eq!(
        err("[workspace]\nmembers = [\"../elsewhere\"]\n"),
        "`[workspace]` `members` lists `../elsewhere`, which isn't a relative path under the workspace root"
    );
    assert_eq!(
        err("[workspace]\nmembers = [\"packages/*\"]\nexclude = [\".\"]\n"),
        "`[workspace]` `exclude` lists `.`, which isn't a relative path under the workspace root"
    );
    assert_eq!(
        err("[workspace]\nmembers = [\"/packages\"]\n"),
        "`[workspace]` `members` lists `/packages`, which isn't a relative path under the workspace root"
    );
    assert_eq!(
        err("[workspace]\nmembers = [\"packages/**\"]\n"),
        "`[workspace]` `members` lists `packages/**` — a pattern matches one directory level per `*`; `**` isn't supported"
    );
    assert_eq!(
        err("[workspace]\nmember = []\n"),
        "`[workspace]` has no key named `member` — did you mean `members`?"
    );
}

#[test]
fn a_workspace_root_is_not_a_project() {
    assert_eq!(
        err("[workspace]\nmembers = [\"a\"]\n\n[project]\nname = \"p\"\n"),
        "a workspace root's `bynk.toml` holds only `[workspace]` and `[registries]` — `[project]` belongs in a member's `bynk.toml`"
    );
    assert_eq!(
        err("[workspace]\nmembers = [\"a\"]\n\n[package]\nname = \"acme/x\"\n"),
        "a workspace root's `bynk.toml` holds only `[workspace]` and `[registries]` — `[package]` belongs in a member's `bynk.toml`"
    );
}

// ---------------------------------------------------------------------------
// Workspaces on disk
// ---------------------------------------------------------------------------

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("packaging_manifest")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// [`check_manifest`] over the project's `bynk.toml` as the CLIs read it: the
/// caller supplies the manifest's text, as `bynk-driver` does.
fn check_on_disk(project: &Path) -> Result<(), ProjectPathsError> {
    let manifest = project.join("bynk.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    check_manifest(project, &HashMap::from([(manifest, text)]))
}

const MEMBER: &str = "[package]\nname = \"acme/m\"\n";

fn workspace(name: &str, root_manifest: &str, members: &[&str]) -> PathBuf {
    let root = scratch(name);
    write(&root.join("bynk.toml"), root_manifest);
    for m in members {
        write(&root.join(m).join("bynk.toml"), MEMBER);
    }
    root
}

fn load_err(root: &Path) -> String {
    load_workspace(root).expect_err("refused").to_string()
}

#[test]
fn members_are_globs_minus_exclude_in_a_stable_order() {
    let root = workspace(
        "globs",
        "[workspace]\nmembers = [\"packages/*\", \"apps/shop\"]\nexclude = [\"packages/scratch\"]\n",
        &[
            "packages/money",
            "packages/fx",
            "packages/scratch",
            "apps/shop",
        ],
    );
    std::fs::create_dir_all(root.join("packages/.hidden")).unwrap();
    let ws = load_workspace(&root).expect("loads");
    let members: Vec<String> = ws
        .members
        .iter()
        .map(|m| m.to_string_lossy().replace('\\', "/"))
        .collect();
    assert_eq!(members, ["apps/shop", "packages/fx", "packages/money"]);
}

#[test]
fn an_empty_workspace_is_refused() {
    let root = workspace("empty", "[workspace]\nmembers = [\"packages/*\"]\n", &[]);
    assert_eq!(
        load_err(&root),
        "`[workspace]` matches no member: no directory under the root matches `packages/*`"
    );
    let root = workspace("empty-list", "[workspace]\nmembers = []\n", &[]);
    assert_eq!(
        load_err(&root),
        "`[workspace]` matches no member: no directory under the root matches nothing (`members` is empty)"
    );
}

#[test]
fn a_member_without_a_manifest_or_a_missing_literal_is_refused() {
    let root = workspace(
        "no-manifest",
        "[workspace]\nmembers = [\"packages/*\"]\n",
        &[],
    );
    std::fs::create_dir_all(root.join("packages/docs")).unwrap();
    assert_eq!(
        load_err(&root),
        "`[workspace]` member `packages/docs` has no `bynk.toml` — add one, or `exclude` it"
    );
    let root = workspace("missing", "[workspace]\nmembers = [\"apps/shop\"]\n", &[]);
    assert_eq!(
        load_err(&root),
        "`[workspace]` `members` lists `apps/shop`, which isn't a directory under the workspace root"
    );
}

#[test]
fn a_workspace_root_holds_no_sources() {
    let root = workspace("sources", "[workspace]\nmembers = [\"a\"]\n", &["a"]);
    write(&root.join("stray.bynk"), "commons stray {}\n");
    assert_eq!(
        load_err(&root),
        "a workspace root holds no sources of its own — move them into a member"
    );
}

#[test]
fn a_members_registries_are_refused() {
    let root = workspace(
        "member-registries",
        "[workspace]\nmembers = [\"packages/*\"]\n",
        &[],
    );
    let member = root.join("packages/money");
    write(
        &member.join("bynk.toml"),
        "[package]\nname = \"acme/money\"\n\n[registries.acme]\nindex = \"https://x\"\n",
    );
    let e = check_on_disk(&member).expect_err("refused");
    assert!(
        matches!(e, ProjectPathsError::MemberRegistries { .. }),
        "{e}"
    );
    let message = e.to_string();
    assert!(
        message.starts_with("`[registries]` belongs in the workspace root's `bynk.toml` (`")
            && message.ends_with("bynk.toml`), not a member's"),
        "{message}"
    );
}

#[test]
fn a_members_own_workspace_is_refused() {
    let root = workspace("nested", "[workspace]\nmembers = [\"packages/*\"]\n", &[]);
    let member = root.join("packages/inner");
    write(
        &member.join("bynk.toml"),
        "[workspace]\nmembers = [\"x\"]\n",
    );
    let e = check_on_disk(&member).expect_err("refused");
    assert!(
        matches!(e, ProjectPathsError::NestedWorkspace { .. }),
        "{e}"
    );
    assert!(
        e.to_string()
            .ends_with("so it can't declare a `[workspace]` of its own"),
        "{e}"
    );
}

#[test]
fn a_nested_unrelated_project_is_not_a_member() {
    // `vendor/tool` sits under the workspace but no pattern lists it, so its
    // own `[registries]` stands.
    let root = workspace(
        "unrelated",
        "[workspace]\nmembers = [\"packages/*\"]\n",
        &["packages/a"],
    );
    let tool = root.join("vendor/tool");
    write(
        &tool.join("bynk.toml"),
        "[project]\nname = \"tool\"\n\n[registries.acme]\nindex = \"https://x\"\n",
    );
    check_on_disk(&tool).expect("not a member");
    // Nor is an excluded one.
    let root = workspace(
        "excluded",
        "[workspace]\nmembers = [\"packages/*\"]\nexclude = [\"packages/scratch\"]\n",
        &["packages/a"],
    );
    let scratch = root.join("packages/scratch");
    write(
        &scratch.join("bynk.toml"),
        "[registries.acme]\nindex = \"https://x\"\n",
    );
    check_on_disk(&scratch).expect("excluded, so not a member");
}

#[test]
fn a_malformed_parent_is_ignored() {
    for (name, parent) in [
        (
            "malformed-parent",
            "[workspace\nmembers = [\"packages/*\"]\n",
        ),
        (
            "unknown-table-parent",
            "[workspace]\nmembers = [\"packages/*\"]\n\n[pahts]\n",
        ),
    ] {
        let root = scratch(name);
        write(&root.join("bynk.toml"), parent);
        let member = root.join("packages/money");
        write(
            &member.join("bynk.toml"),
            "[package]\nname = \"acme/money\"\n\n[registries.acme]\nindex = \"https://x\"\n",
        );
        check_on_disk(&member)
            .unwrap_or_else(|e| panic!("{name}: a broken parent counts as absent, but: {e}"));
    }
}

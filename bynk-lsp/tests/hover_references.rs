//! #611/#616: hover on a name's *references*, not just its declaration.
//!
//! #611 — hover worked on `store`/`key` declarations but not on the uses in an
//! agent handler body: a store-field reference, a record-construction field
//! label, and a store method call each resolved nowhere (or, for a label,
//! mis-bound to a same-named handler param). Every hover test before this one
//! used declaration offsets, which is why the gaps slipped through — so these
//! fixtures are pinned at *reference* offsets in `examples/todo/src/todos.bynk`,
//! the file the issue reproduces in, against real `diagnose_project` output.
//!
//! #616 (v0.166, ADR 0191) — the same blind spot, three kinds further out.
//! `Actor`/`Method`/`CapabilityOp` resolved through the index and rendered by
//! nothing, and ADR 0190 D1 recorded them as hovering as *nothing* on a
//! measurement taken from the ladder's tail. At a reference offset two of them
//! render the **wrong** declaration instead (the fall-through passes the lexical
//! name match first), so each fixture below pins the wrong answer it replaces —
//! not just the right one. A test asserting only `Gauge.bump` would pass against
//! a renderer that had never been broken, and this one was.
//!
//! The `Method`/`CapabilityOp` fixtures declare things `examples/todo` has none
//! of, so they borrow the compiler's own positive fixtures — real projects,
//! analysed the same way.
//!
//! bynk-lsp is a binary crate: include the pure modules directly (the pattern
//! `legend_drift.rs` established). `Backend::hover` is transport and cannot be
//! called from here — but the resolution *ladder* it runs is
//! [`hover::hover_content`], which is pure, so [`hover_at`] calls **the real
//! thing**. Reordering the rungs breaks these tests, which is the point: gap B
//! was a fall-through bug, so the ordering is the behaviour under test, and a
//! replica of it would agree with the original only until one of them changed.
//!
//! This file is `exclude`d from the published `bynk-lsp` crate (Cargo.toml) —
//! `examples/todo` and the compiler's fixtures live outside this crate's
//! package, so a standalone `cargo test` on the published crate must not see
//! it. Skipping when a root is absent is not the alternative: ADR 0190 D6
//! requires these to read real output, and a skip could pass vacuously in-repo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_ide::ProjectDiagnostics;
use bynk_lsp::hover;
use tower_lsp::lsp_types::Url;

/// The analysed `examples/todo` project — the issue's reproduction. The
/// fourth element is the project root `hover_at` needs to build a #848
/// doc-link's `file://` target.
fn todos() -> (ProjectDiagnostics, PathBuf, String, PathBuf) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/todo/src");
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let r = bynk_ide::diagnose_project(&root, &sources);
    let file = r
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with("todos.bynk"))
        .expect("todos.bynk analysed");
    let (path, text) = (file.source_path.clone(), file.text.clone());
    (r, path, text, root)
}

/// v0.166 (#616): any project under `<crate>/{rel}` analysed, with the cursor
/// file's index-relative path and text, plus the project root (see [`todos`]).
/// The `Method`/`CapabilityOp` fixtures need declarations `examples/todo` has
/// none of, so they borrow the compiler's own positive fixtures — real
/// projects on disk, analysed the same way.
fn analysed(rel: &str, file: &str) -> (ProjectDiagnostics, PathBuf, String, PathBuf) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let r = bynk_ide::diagnose_project(&root, &sources);
    let f = r
        .files
        .iter()
        .find(|f| f.source_path.ends_with(file))
        .unwrap_or_else(|| panic!("{file} analysed"));
    let (path, text) = (f.source_path.clone(), f.text.clone());
    let key = index_key(&r, &path);
    (r, key, text, root)
}

/// The byte offset of `needle` at or after `anchor` — the reference site, as
/// distinct from the declaration of the same name earlier in the file.
fn at(text: &str, anchor: &str, needle: &str) -> usize {
    let a = text
        .find(anchor)
        .unwrap_or_else(|| panic!("`{anchor}` is not in the fixture"));
    a + text[a..]
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is not at `{anchor}`"))
}

/// The project-relative path the index keys spans against (the index and the
/// `files` list can disagree on absolute-vs-relative; match by file name).
fn index_path(r: &ProjectDiagnostics) -> PathBuf {
    r.index
        .symbols
        .values()
        .filter_map(|e| e.def.as_ref())
        .map(|d| d.path.clone())
        .find(|p| p.to_string_lossy().ends_with("todos.bynk"))
        .expect("todos.bynk is in the index")
}

/// Drive **the real hover ladder** (`Backend::hover`'s pure core) over analysed
/// project output. The snapshot and the live buffer are the same text here — the
/// file is on disk and unedited — which is exactly the steady state a saved
/// document is in.
fn hover_at(
    r: &ProjectDiagnostics,
    rel: &Path,
    text: &str,
    root: &Path,
    offset: usize,
) -> Option<String> {
    let snapshots: HashMap<PathBuf, String> = r
        .files
        .iter()
        .map(|f| (index_key(r, &f.source_path), f.text.clone()))
        .collect();
    let uri = Url::parse("file:///todos.bynk").unwrap();
    hover::hover_content(&hover::HoverInput {
        analysis: Some(hover::HoverAnalysis {
            index: &r.index,
            snapshots: &snapshots,
            locals: &r.locals,
            expr_types: &r.expr_types,
            tys: &r.ty_intern,
            rel,
            offset,
            project_root: root,
            doc_scope: &r.doc_scope,
            boundary_info: &r.boundary_info,
            context_count: bynk_ide::wire_contract::real_context_count(
                &r.boundary_info,
                &r.unit_sources,
            ),
        }),
        doc: Some((text, offset)),
        uri: &uri,
        // No src_root: the cross-file / first-party rungs are not what these
        // fixtures exercise, and `examples/todo` is single-file.
        files: None,
    })
}

/// The path the round's tables key `p` under (the index/locals/expr_types maps
/// and the `files` list can disagree on absolute-vs-relative; match by name).
fn index_key(r: &ProjectDiagnostics, p: &Path) -> PathBuf {
    let keys = || {
        r.locals.keys().chain(
            r.index
                .symbols
                .values()
                .filter_map(|e| e.def.as_ref())
                .map(|d| &d.path),
        )
    };
    // A whole-path suffix match first, then the path itself when two
    // analysed files share its name (`src/cr/jobs.bynk`, `tests/jobs.bynk`),
    // so they key apart; otherwise by name.
    let shared_name = r
        .files
        .iter()
        .filter(|f| f.source_path.file_name() == p.file_name())
        .count()
        > 1;
    keys()
        .find(|k| p.ends_with(k) || k.ends_with(p))
        .cloned()
        .or_else(|| shared_name.then(|| p.to_path_buf()))
        .or_else(|| keys().find(|k| k.file_name() == p.file_name()).cloned())
        .unwrap_or_else(|| p.to_path_buf())
}

/// The byte offset of `needle` within the `add` handler body — the issue's
/// reproduction site. Anchored to the handler so a `needle` that also occurs
/// earlier in the file (`items` declares before it is used) resolves to the use.
fn in_add_handler(text: &str, needle: &str) -> usize {
    let body = text.find("on call add(").expect("the `add` handler");
    let end = text[body..].find("\n  }").expect("the handler's end") + body;
    body + text[body..end].find(needle).unwrap_or_else(|| {
        panic!(
            "`{needle}` is not in the `add` handler body:\n{}",
            &text[body..end]
        )
    })
}

/// Gap A — a `store`-field reference (`lastSeq` in `let next = lastSeq + 1`)
/// hovers as its `store lastSeq: Cell[Int]` declaration. Observed: nothing.
#[test]
fn store_field_reference_hovers_as_its_declaration() {
    let (r, _, text, root) = todos();
    let rel = index_path(&r);
    let hover = hover_at(&r, &rel, &text, &root, in_add_handler(&text, "lastSeq + 1"))
        .expect("hover on the `lastSeq` reference");
    assert!(hover.contains("store lastSeq: Cell[Int]"), "{hover}");

    // The declaration hovered before this change and still does — identically.
    let decl = hover_at(&r, &rel, &text, &root, text.find("store lastSeq").unwrap())
        .expect("hover on the declaration");
    assert_eq!(hover, decl);
}

/// Gap B — record-construction field labels (`Stored { seq: …, title: …, done:
/// … }`) hover as the `Stored` type's fields. Observed: `title:` showed the
/// handler param `title`; `seq:`/`done:` showed nothing.
#[test]
fn record_construction_labels_hover_as_the_types_fields() {
    let (r, _, text, root) = todos();
    let rel = index_path(&r);
    let ctor = in_add_handler(&text, "Stored { seq: next");

    for (label, expected) in [
        ("seq: next", "seq: Int"),
        ("title: title", "title: Title"),
        ("done: false", "done: Bool"),
    ] {
        let offset = ctor + text[ctor..].find(label).expect("the label");
        let hover = hover_at(&r, &rel, &text, &root, offset)
            .unwrap_or_else(|| panic!("no hover on the `{label}` label"));
        assert!(
            hover.contains(expected) && hover.contains("A field of `Stored`"),
            "`{label}` should describe the `Stored` field, got:\n{hover}"
        );
    }

    // The regression the fall-through caused: `title:` bound to the enclosing
    // `add(title: Title)` param. The param itself still hovers as a param.
    let param = text
        .find("title: Title) -> Effect")
        .expect("the handler param");
    let hover = hover_at(&r, &rel, &text, &root, param).expect("hover on the param");
    assert!(
        hover.contains("param"),
        "the param still hovers as one: {hover}"
    );
}

/// Gap C — a store method call (`items.put(id, item)`) hovers: the receiver as
/// the `store items` field, the operation as its signature. Observed: nothing.
#[test]
fn store_method_call_hovers_receiver_and_operation() {
    let (r, _, text, root) = todos();
    let rel = index_path(&r);
    let call = in_add_handler(&text, "items.put(id, item)");

    let recv = hover_at(&r, &rel, &text, &root, call).expect("hover on the `items` receiver");
    assert!(recv.contains("store items: Map[String, Stored]"), "{recv}");

    let op = hover_at(&r, &rel, &text, &root, call + "items.".len()).expect("hover on `put`");
    assert!(op.contains("put(key: K, value: V) -> Effect[()]"), "{op}");
    assert!(op.contains("store items: Map[String, Stored]"), "{op}");
}

/// The rung *order* is the contract, stated rather than left implicit.
///
/// Gap B was a fall-through: a rung that resolved the offset correctly but
/// rendered nothing let a later name-matching rung answer. These two offsets are
/// the ones where the rungs actively disagree, so they pin the precedence:
/// hoisting the locals rung above the index rung reintroduces #611 and fails
/// here — which is what makes calling the real `hover_content` (rather than a
/// replica of it) load-bearing.
#[test]
fn a_structural_rung_outranks_the_name_matching_locals_rung() {
    let (r, _, text, root) = todos();
    let rel = index_path(&r);

    // `title:` — the index resolves `Stored.title`; the locals rung would match
    // the in-scope `add(title: Title)` param by name. The index must win.
    let ctor = in_add_handler(&text, "Stored { seq: next");
    let label = ctor + text[ctor..].find("title: title").expect("the label");
    let hover = hover_at(&r, &rel, &text, &root, label).expect("hover on the label");
    assert!(
        hover.contains("A field of `Stored`") && !hover.contains("param"),
        "the index rung must outrank the locals rung, got:\n{hover}"
    );

    // `put` — the store-op rung matches structurally off the declared field. No
    // local named `put` exists here, so this pins the rung is reached at all
    // (it sits between the index and locals rungs).
    let call = in_add_handler(&text, "items.put(id, item)");
    let op = hover_at(&r, &rel, &text, &root, call + "items.".len()).expect("hover on `put`");
    assert!(op.contains("store operation"), "{op}");
}

/// #616 — an `actor` reference (`by u: User`) hovers as its declaration.
/// Observed: nothing at all, at the reference *and* the declaration. The index
/// resolved the `Actor` key both times; `describe_item` had no arm for it, and
/// no later rung knows what an actor is.
#[test]
fn actor_reference_hovers_as_its_declaration() {
    let (r, rel, text, root) = analysed("../examples/todo/src", "todos.bynk");
    let hover = hover_at(&r, &rel, &text, &root, at(&text, "by u: User", "User"))
        .expect("hover on the `User` actor reference");
    assert!(
        hover.contains(
            "actor User { auth = Bearer(secret = \"AUTH_JWT_SECRET\"), identity = UserId }"
        ),
        "{hover}"
    );

    // ADR 0190 D2: a reference answers the same question as its declaration, so
    // it renders the same content, from the same builder.
    let decl = hover_at(&r, &rel, &text, &root, at(&text, "actor User", "User"))
        .expect("hover on the declaration");
    assert_eq!(hover, decl);
}

/// #616 — a method reference hovers the method the index *bound* it to, not the
/// first one that happens to share its spelling.
///
/// This is #611's gap B exactly: the index resolved `Gauge.bump`, the renderer
/// had no `Method` arm, and the lexical rung answered by name — matching a
/// method on its bare `bump` and rendering `Counter.bump`, the first `bump`
/// declared. `fn Gauge.bump`'s own declaration hovered as `Counter.bump` too.
#[test]
fn method_reference_hovers_the_bound_method_not_a_same_named_one() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/216_method_index/src",
        "shop.bynk",
    );

    // The wrong-answer case: `Gauge.bump` is declared second, so a name match
    // renders `Counter.bump` — confidently, and wrongly.
    for (anchor, expect, not) in [
        ("g.bump()", "fn Gauge.bump(self) -> Gauge", "Counter"),
        ("c.bump()", "fn Counter.bump(self) -> Counter", "Gauge"),
        ("fn Gauge.bump", "fn Gauge.bump(self) -> Gauge", "Counter"),
    ] {
        let hover = hover_at(&r, &rel, &text, &root, at(&text, anchor, "bump"))
            .unwrap_or_else(|| panic!("no hover on `bump` at `{anchor}`"));
        assert!(
            hover.contains(expect) && !hover.contains(not),
            "`{anchor}` should hover `{expect}`, got:\n{hover}"
        );
    }
}

/// #616 — a capability operation hovers the project's *own* op, attributed to
/// its capability.
///
/// The name-match this replaces reached past the project into the **embedded**
/// first-party surface: `bynk.bynk` declares `platform.log.Logger.info(msg:
/// String)`, so a project that declares its own `Logger.info(message: String)`
/// hovered the stdlib op that merely shares the spelling — a different
/// parameter, a different owner.
#[test]
fn capability_op_reference_hovers_the_projects_own_op() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/160_provider_given_basic/src",
        "demo.bynk",
    );

    let hover = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "Logger.info(\"hello\")", "info"),
    )
    .expect("hover on the `Logger.info` reference");
    assert!(
        hover.contains("fn info(message: String) -> Effect[()]")
            && hover.contains("An operation of capability `Logger`"),
        "{hover}"
    );
    assert!(
        !hover.contains("msg: String"),
        "the embedded `platform.log.Logger.info(msg)` must not answer for a \
         project capability that shares its name:\n{hover}"
    );

    // ADR 0190 D2, as for the actor above.
    let decl = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "capability Logger { fn info", "info"),
    )
    .expect("hover on the declaration");
    assert_eq!(hover, decl);
}

/// #855 — the new rung 4 (the handler-HEADER rung) answers an offset no
/// earlier rung ever did, and its guard provably cannot shadow rung 1's
/// resolution of a param's *type* name — the safety property the phase's
/// plan calls out as the highest-risk piece of this change.
#[test]
fn header_rung_answers_the_header_and_never_shadows_a_params_type_name() {
    let (r, rel, text, root) = analysed("../examples/rate-limiter/src", "ratelimit.bynk");

    // `GET` in `on GET("/check/:client")` — inside the header's byte range
    // (before the first param's span) — resolves through no rung before
    // #855. Now it answers the handler's wire contract.
    let header = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "on GET(\"/check/:client\")", "GET"),
    )
    .expect("hover on the handler header must answer via rung 4");
    assert!(
        header.contains("Wire contract") && header.contains("GET /check/:client"),
        "the header rung should render the handler's wire contract, got:\n{header}"
    );
    assert!(
        header.contains("the request body **is** the value of `client`"),
        "a single-param handler is the bare envelope, got:\n{header}"
    );

    // The route literal itself, inside the header range too.
    let route = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "on GET(\"/check/:client\")", "/check/:client"),
    )
    .expect("hover on the route literal must answer via rung 4");
    assert!(route.contains("Wire contract"), "{route}");

    // `ClientId` in `(client: ClientId)` — past the header's guarded range
    // (it's the first param's *type*, not the header) — must still resolve
    // through rung 1 to the type's own declaration, exactly as it did before
    // #855. This is the shadowing invariant: the new rung's guard is `h.span
    // .start <= offset && offset < prefix_end`, and a param's type name
    // always sits at or after `prefix_end`.
    let param_type = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "(client: ClientId) -> Effect[HttpResult", "ClientId"),
    )
    .expect("hover on the param's type name");
    assert!(
        param_type.contains("type ClientId") && !param_type.contains("**Wire contract**"),
        "the param's type name must still resolve to its own declaration \
         (rung 1), not the new header rung, got:\n{param_type}"
    );
}

/// #848 — a resolvable intra-doc link in a declaration's doc comment renders
/// as a Markdown link in hover, not literal brackets. `ClientId`'s doc
/// references `[Limiter]`, a bare name declared in the same unit.
#[test]
fn hover_renders_a_resolved_intra_doc_link() {
    let (r, rel, text, root) = analysed("../examples/rate-limiter/src", "ratelimit.bynk");
    let hover = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "type ClientId", "ClientId"),
    )
    .expect("hover on the `ClientId` declaration");
    assert!(
        hover.contains("[Limiter](file://") && hover.contains("ratelimit.bynk"),
        "the doc's `[Limiter]` reference should render as a link, got:\n{hover}"
    );
}

/// #2455 — a first-party capability op (`[Clock.now]` in the fixture's `api`
/// service doc) renders as a link to the Book, not a file.
#[test]
fn hover_links_a_first_party_name_to_the_book() {
    let (r, rel, text, root) = analysed("../examples/rate-limiter/src", "ratelimit.bynk");
    let hover = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "service api from http", "api"),
    )
    .expect("hover on the `api` service declaration");
    assert!(
        hover.contains("[Clock.now](https://bynk-lang.org/book/reference/bynk-capabilities/#"),
        "a first-party doc link should render as a Book link, got:\n{hover}"
    );
}

/// #2460 — hovering a resolved `[decide]` in a doc comment previews the
/// target exactly as hovering the name in code would.
#[test]
fn hover_on_a_resolved_doc_link_previews_its_target() {
    let (r, rel, text, root) = analysed("../examples/rate-limiter/src", "ratelimit.bynk");
    let hover = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "[decide] for how", "decide"),
    )
    .expect("hover on the `[decide]` doc link");
    assert!(
        hover.contains("fn decide(prevStart: Int, prevCount: Int, now: Int, windowMs: Int, limit: Int) -> Decision"),
        "the link's target signature should be previewed, got:\n{hover}"
    );
    assert!(
        !hover.contains("Unresolved"),
        "a resolvable link must not report unresolved, got:\n{hover}"
    );
}

/// #2460 — a dotted `[`RateView.remaining`]` link previews its member.
#[test]
fn hover_on_a_resolved_dotted_doc_link_previews_its_member() {
    let (r, rel, text, root) = analysed("../examples/rate-limiter/src", "ratelimit.bynk");
    let hover = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "RateView.remaining`]", "remaining"),
    )
    .expect("hover on the dotted doc link");
    assert!(
        hover.contains("remaining: Int") && !hover.contains("Unresolved"),
        "got:\n{hover}"
    );
}

/// #2460 — an unresolved link says so and lists the scope searched.
#[test]
fn hover_on_an_unresolved_doc_link_says_so() {
    let (r, rel, text, root) = analysed("../examples/rate-limiter/src", "ratelimit.bynk");
    let hover = hover_at(&r, &rel, &text, &root, at(&text, "[Clock.now]", "Clock"))
        .expect("hover on the unresolved `[Clock.now]` doc link");
    assert!(
        hover.contains("Unresolved doc link `Clock.now`") && hover.contains("ratelimit"),
        "got:\n{hover}"
    );
}

/// #2460 — doc-comment prose outside any `[...]` span is not a doc-link
/// hover: rung 4b declines and the ladder answers as it did before.
#[test]
fn hover_on_doc_prose_outside_a_link_is_not_a_doc_link_hover() {
    let (r, rel, text, root) = analysed("../examples/rate-limiter/src", "ratelimit.bynk");
    let hover = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "for how the window", "window"),
    );
    assert!(
        hover.is_none_or(|h| !h.contains("doc link")),
        "prose outside a link must not produce a doc-link hover"
    );
}

/// The example this issue reproduces in must stay clean — every assertion above
/// reads real analysis output, which a broken fixture would silently empty.
#[test]
fn the_todo_example_analyses_cleanly() {
    let (r, _, _, _) = todos();
    let errors: Vec<_> = r
        .files
        .iter()
        .flat_map(|f| f.diagnostics.iter())
        .map(|d| d.error.category)
        .collect();
    assert!(
        errors.is_empty(),
        "examples/todo has diagnostics: {errors:?}"
    );
    assert!(!r.index.symbols.is_empty(), "the index is populated");
}

/// #1888 — a documented record field hovers with its own doc in place of "A
/// field of `Run`.", wherever field hover resolves: at its declaration, at a
/// field access (`r.repo`) and at a record-construction label. Read from the
/// real analysis of the `1888_one_line_docs` fixture; the offsets are found
/// by text, so the re-parsed spans' `FileId` never enters into it.
#[test]
fn a_documented_field_hovers_with_its_doc_everywhere() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/1888_one_line_docs/src",
        "model.bynk",
    );
    for (anchor, needle) in [
        ("\trepo: String", "repo"),
        ("r.repo", "repo"),
        ("Run { repo: repo", "repo"),
    ] {
        let hover = hover_at(&r, &rel, &text, &root, at(&text, anchor, needle))
            .unwrap_or_else(|| panic!("no hover on `{needle}` at `{anchor}`"));
        assert!(
            hover.contains("repo: String")
                && hover.contains("The repository, as `owner/name`.")
                && !hover.contains("A field of"),
            "`{anchor}` should hover the documented field, got:\n{hover}"
        );
    }
    // A `---` doc on a field shows the same way; a `--` comment above one is
    // not a doc, so that field keeps the generic line.
    let version = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "version: \"1.0\"", "version"),
    )
    .expect("hover on the `version` label");
    assert!(
        version.contains("The Bynk release the checks ran against."),
        "{version}"
    );
    let passed = hover_at(&r, &rel, &text, &root, at(&text, "passed: 0", "passed"))
        .expect("hover on the `passed` label");
    assert!(
        passed.contains("A field of `Run`.") && !passed.contains("comment"),
        "{passed}"
    );
}

/// #1888 — a sum variant hovers with its doc, and its documented payload
/// field's: at its declaration and as a match pattern.
#[test]
fn a_documented_variant_hovers_with_its_doc() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/1888_one_line_docs/src",
        "model.bynk",
    );
    for anchor in ["| Circle(", "Circle(radius) =>"] {
        let hover = hover_at(&r, &rel, &text, &root, at(&text, anchor, "Circle"))
            .unwrap_or_else(|| panic!("no hover on `Circle` at `{anchor}`"));
        assert!(
            hover.contains("| Circle(radius: Int)")
                && hover.contains("A circle.")
                && hover.contains("`radius`: The radius, in pixels."),
            "`{anchor}` should hover the documented variant, got:\n{hover}"
        );
    }
    let pass = hover_at(&r, &rel, &text, &root, at(&text, "\tPass,", "Pass"))
        .expect("hover on the `Pass` declaration");
    assert!(pass.contains("Every canary check passed."), "{pass}");
}

/// #1883 — an asset reference hovers as its declaration: the declaration, where
/// the platform serves the file, and its doc. Across a `uses` edge too, which
/// is the common case (a commons declares the asset, a context uses it).
#[test]
fn asset_reference_hovers_as_its_declaration() {
    // Analysed from `src/`, so the asset files beside `bynk.toml` are not
    // found (`bynk.asset.missing_file`); hover reads the declaration, never
    // the file, so it answers all the same.
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/1883_asset_workers/src",
        "board.bynk",
    );
    let hover = hover_at(&r, &rel, &text, &root, at(&text, "style.url", "style"))
        .expect("hover on the `style` asset reference");
    for want in [
        "asset style = \"assets/board.css\"",
        "Served at `/_assets/board.<hash>.css`",
        "The board's stylesheet.",
    ] {
        assert!(hover.contains(want), "missing `{want}` in:\n{hover}");
    }

    // ADR 0190 D2: the declaration renders the same content.
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/1883_asset_workers/src",
        "page.bynk",
    );
    let decl = hover_at(&r, &rel, &text, &root, at(&text, "asset style", "style"))
        .expect("hover on the declaration");
    assert_eq!(hover, decl);
}

/// #1883 — an asset reference is coloured as an asset: the `asset` token type
/// (legend index 13), not a variable or a function.
#[test]
fn asset_reference_gets_the_asset_semantic_token() {
    let (r, rel, text, _) = analysed(
        "../bynkc/tests/fixtures/positive/1883_asset_workers/src",
        "board.bynk",
    );
    let legend = bynk_lsp::index_queries::semantic_tokens_legend();
    let asset = legend
        .token_types
        .iter()
        .position(|t| t.as_str() == "asset")
        .expect("`asset` is in the legend") as u32;
    let tokens = bynk_lsp::index_queries::semantic_tokens(&r.index, &[], &[], &rel, &text, None);
    // Decode the LSP delta encoding to absolute (line, column) positions.
    let off = at(&text, "style.url", "style");
    let want_line = text[..off].matches('\n').count() as u32;
    let want_col = (off - text[..off].rfind('\n').map_or(0, |n| n + 1)) as u32;
    let (mut line, mut col) = (0u32, 0u32);
    let mut found = Vec::new();
    for t in &tokens {
        if t.delta_line > 0 {
            line += t.delta_line;
            col = t.delta_start;
        } else {
            col += t.delta_start;
        }
        if (line, col) == (want_line, want_col) {
            found.push((t.length, t.token_type));
        }
    }
    assert_eq!(found, vec![("style".len() as u32, asset)]);
}

/// #2387 — an embed reference hovers as its declaration: the declaration, that
/// it is private, and its doc; the same content at a use in another unit and
/// at the declaration (ADR 0190 D2).
#[test]
fn embed_reference_hovers_as_its_declaration() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/2387_embed_unit/src",
        "web.bynk",
    );
    let hover = hover_at(&r, &rel, &text, &root, at(&text, "Ok(shell)", "shell"))
        .expect("hover on the `shell` embed reference");
    for want in [
        "embed shell = \"embeds/page.html\"",
        "Private: the file's text, compiled into the code as a `String`; never served.",
        "The page shell: a project file's text, compiled in and never served.",
    ] {
        assert!(hover.contains(want), "missing `{want}` in:\n{hover}");
    }

    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/2387_embed_unit/src",
        "page.bynk",
    );
    let decl = hover_at(&r, &rel, &text, &root, at(&text, "embed shell", "shell"))
        .expect("hover on the declaration");
    assert_eq!(hover, decl);
}

/// #2387 — an embed reference is coloured as an embed: the `embed` token type
/// (legend index 14), not a variable or a function.
#[test]
fn embed_reference_gets_the_embed_semantic_token() {
    let (r, rel, text, _) = analysed(
        "../bynkc/tests/fixtures/positive/2387_embed_unit/src",
        "web.bynk",
    );
    let legend = bynk_lsp::index_queries::semantic_tokens_legend();
    let embed = legend
        .token_types
        .iter()
        .position(|t| t.as_str() == "embed")
        .expect("`embed` is in the legend") as u32;
    let tokens = bynk_lsp::index_queries::semantic_tokens(&r.index, &[], &[], &rel, &text, None);
    let off = at(&text, "Ok(shell)", "shell");
    let want_line = text[..off].matches('\n').count() as u32;
    let want_col = (off - text[..off].rfind('\n').map_or(0, |n| n + 1)) as u32;
    let (mut line, mut col) = (0u32, 0u32);
    let mut found = Vec::new();
    for t in &tokens {
        if t.delta_line > 0 {
            line += t.delta_line;
            col = t.delta_start;
        } else {
            col += t.delta_start;
        }
        if (line, col) == (want_line, want_col) {
            found.push((t.length, t.token_type));
        }
    }
    assert_eq!(found, vec![("shell".len() as u32, embed)]);
}

/// #1883 — an asset with an `at` path hovers as served at that fixed path,
/// never at a content-hashed `/_assets/` one (review of #1924).
#[test]
fn fixed_path_asset_hovers_its_at_path() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/1883_asset_fixed_paths_workers/src",
        "board.bynk",
    );
    let hover = hover_at(&r, &rel, &text, &root, at(&text, "favicon.url", "favicon"))
        .expect("hover on the `favicon` asset reference");
    assert!(
        hover.contains("asset favicon = \"assets/favicon.ico\" at \"/favicon.ico\""),
        "{hover}"
    );
    assert!(hover.contains("Served at `/favicon.ico`."), "{hover}");
    assert!(!hover.contains("/_assets/"), "{hover}");
}

/// S4a (#1966): a derived capability (`capability Sessions = Kv`) declares no
/// operations of its own; it has its base's. Hover on the declaration says it
/// is a distinct instance of the base, and on an operation reference renders
/// the base's operation.
#[test]
fn derived_capability_hovers_as_an_instance_of_its_base() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/1966_derived_kv_workers/src",
        "cart.bynk",
    );

    let decl = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "capability Sessions", "Sessions"),
    )
    .expect("hover on the declaration");
    assert!(decl.contains("capability Sessions = Kv"), "{decl}");
    assert!(
        decl.contains("A distinct instance of `Kv` (`bynk.cloudflare`)"),
        "{decl}"
    );
    assert!(
        decl.contains("fn put(key: String, value: String) -> Effect[()]"),
        "the base's operations: {decl}"
    );
    assert!(decl.contains("Live carts, keyed by shopper."), "{decl}");

    // The base is `bynk.cloudflare`'s `Kv`, not the instance.
    let base = hover_at(&r, &rel, &text, &root, at(&text, "Sessions = Kv", "Kv"))
        .expect("hover on the base");
    assert!(base.contains("capability Kv {"), "{base}");

    let given = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "given Sessions", "Sessions"),
    )
    .expect("hover on a `given` reference");
    assert!(given.contains("capability Sessions = Kv"), "{given}");

    let op = hover_at(&r, &rel, &text, &root, at(&text, "Sessions.get(k)", "get"))
        .expect("hover on the `Sessions.get` reference");
    assert!(
        op.contains("get(key: String) -> Effect[Option[String]]"),
        "{op}"
    );
}

/// #2087: a service whose contract names a type `contract_types_for` filed
/// under a qualified key (`shop_order__Code`, `shop.order`'s `Code`, borrowed
/// into `shop.ledger`, which declares a `Code` of its own) shows it in the
/// hover's contract by its declaration's own name.
#[test]
fn hover_shows_a_qualified_key_by_its_declarations_name() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/2087_consumed_opaque_qualified_key",
        "ledger.bynk",
    );
    let h = hover_at(
        &r,
        &rel,
        &text,
        &root,
        at(&text, "service orderId", "orderId"),
    )
    .expect("hover on the service");
    assert!(h.contains("Cross-context contract"), "{h}");
    assert!(h.contains("Code"), "{h}");
    assert!(!h.contains("shop_order__"), "a qualified key leaked: {h}");
}

/// #2136 (ADR 0156): in a `system`-tier case, hovering the `schedule` or
/// `message` address name says it runs through the Worker's own entry point;
/// the same address in an `integration` case says nothing of the kind.
#[test]
fn system_cron_and_queue_addresses_hover_as_worker_entries() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/2136_system_cron_queue",
        "tests/jobs.bynk",
    );
    let (r, rel, text, root) = (&r, rel.as_path(), text.as_str(), root.as_path());
    let schedule = hover_at(r, rel, text, root, at(text, "sweeper.schedule", "schedule"))
        .expect("`schedule` hovers");
    assert!(schedule.contains("`scheduled` entry"), "{schedule}");
    let message = hover_at(
        r,
        rel,
        text,
        root,
        at(text, "deliveries.message", "message"),
    )
    .expect("`message` hovers");
    assert!(message.contains("`queue` entry"), "{message}");

    let (r2, rel2, text2, root2) = analysed(
        "../bynkc/tests/fixtures/positive/2131_cron_queue_address_in_process",
        "tests/jobs.bynk",
    );
    let in_process = hover_at(
        &r2,
        &rel2,
        &text2,
        &root2,
        at(&text2, "sweeper.schedule", "schedule"),
    );
    assert!(
        in_process.is_none_or(|h| !h.contains("`scheduled` entry")),
        "an `integration` case runs the handler in process"
    );
}

/// #2138 (ADR 0156): through the whole hover dispatch, `run.id` in a case
/// body describes the test run's identity, at `system` and at `unit`; where
/// the case binds its own `run`, `run.id` is that value's field instead.
#[test]
fn run_id_hovers_as_the_test_run() {
    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/2138_run_id",
        "tests/system.bynk",
    );
    let (r, rel, text, root) = (&r, rel.as_path(), text.as_str(), root.as_path());
    let h = hover_at(r, rel, text, root, at(text, "\\(run.id", "id"))
        .expect("`run.id` in an interpolation hovers");
    assert!(h.contains("test run's identity"), "{h}");

    let (r, rel, text, root) = analysed(
        "../bynkc/tests/fixtures/positive/2138_run_id",
        "tests/unit.bynk",
    );
    let (r, rel, text, root) = (&r, rel.as_path(), text.as_str(), root.as_path());
    let h = hover_at(r, rel, text, root, at(text, "run.id ==", "run")).expect("`run` hovers");
    assert!(h.contains("test run's identity"), "{h}");
    let shadowed = hover_at(r, rel, text, root, at(text, "run.id == \"mine\"", "run"));
    assert!(
        shadowed.is_none_or(|h| !h.contains("test run's identity")),
        "a bound `run` is an ordinary value"
    );
}

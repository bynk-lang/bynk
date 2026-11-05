//! What a name resolves to across `uses`, `consumes`, shadowing and test
//! suites (#1833).
//!
//! Each [`Row`] is a small multi-unit project and what the checker must
//! decide about it, asserted through the checker's own records rather than
//! through emitted text: the declaration a reference resolved to (the binding
//! index), the type an expression got (the expression-type table), and which
//! diagnostics fire. Adding a case is adding a row; no fixture directory.
//!
//! Two expectations need care:
//!
//! - [`Expect::Kind`] exists because two same-named types display the same.
//!   A local `type Repo = Int` and a used commons'
//!   `type Repo = String where NonEmpty` both display as `Repo`; their
//!   [`NamedKind`]s differ.
//! - Expression types are not recorded for test-suite files, so a suite row
//!   asserts through [`Expect::Reports`] instead.
//!
//! **Known defects.** A row that pins an open bug is not in [`ROWS`]. It has
//! its own `#[should_panic]` test naming the row, so the suite stays green
//! while the bug is open and fails the moment it is fixed: the fixing PR moves
//! the row into [`ROWS`] and deletes the `#[should_panic]` test, the same
//! strict-xfail discipline as `bynk-tests/tests/behaviour_fixtures.rs`.

use crate::checker::{NamedKind, Ty};
use crate::testkit::{Analysed, analyse};
use bynk_syntax::ast::BaseType;

/// One thing the checker must have decided. `at` locates the site by the
/// first occurrence of its text in `file`.
enum Expect {
    /// The reference at `at` resolved to a declaration in `unit`.
    Resolves {
        file: &'static str,
        at: &'static str,
        unit: &'static str,
    },
    /// The expression spelled `at` has the displayed type `ty`.
    Type {
        file: &'static str,
        at: &'static str,
        ty: &'static str,
    },
    /// The expression spelled `at` has a named type of this kind.
    Kind {
        file: &'static str,
        at: &'static str,
        kind: NamedKind,
    },
    /// Some diagnostic has this category.
    Reports(&'static str),
    /// No diagnostics at all.
    Clean,
}

struct Row {
    /// Unique; a `#[should_panic]` known-defect test matches on it.
    name: &'static str,
    files: &'static [(&'static str, &'static str)],
    expect: &'static [Expect],
}

/// Check every expectation of `row`, then panic listing every one that
/// failed, prefixed `row `<name>``.
fn check(row: &Row) {
    let a = analyse(row.files);
    let failures: Vec<String> =
        row.expect
            .iter()
            .filter_map(|e| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| verify(&a, e)))
                    .unwrap_or_else(|p| {
                        Some(
                            p.downcast_ref::<String>()
                                .cloned()
                                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                                .unwrap_or_default(),
                        )
                    })
            })
            .collect();
    assert!(
        failures.is_empty(),
        "row `{}`:\n- {}\ndiagnostics:\n{}",
        row.name,
        failures.join("\n- "),
        a.render()
    );
}

/// `None` when `e` holds, otherwise what was found instead.
fn verify(a: &Analysed, e: &Expect) -> Option<String> {
    match e {
        Expect::Resolves { file, at, unit } => {
            let got = &a.resolves_to(file, at).unit;
            (got != unit)
                .then(|| format!("`{at}` in {file} resolves to `{got}`, expected `{unit}`"))
        }
        Expect::Type { file, at, ty } => {
            let got = a.type_at(file, at);
            (got != *ty).then(|| format!("`{at}` in {file} has type `{got}`, expected `{ty}`"))
        }
        Expect::Kind { file, at, kind } => match a.ty_at(file, at) {
            Ty::Named { kind: got, .. } if got == *kind => None,
            got => Some(format!(
                "`{at}` in {file} has type {got:?}, expected kind {kind:?}"
            )),
        },
        Expect::Reports(category) => {
            (!a.categories().contains(category)).then(|| format!("no `{category}` diagnostic"))
        }
        Expect::Clean => {
            (!a.categories().is_empty()).then(|| "expected no diagnostics".to_string())
        }
    }
}

const CORE: &str = "commons t.core\n\ntype Repo = String where NonEmpty\n";

/// `t.model` uses `t.core`; its `Run` reaches `t.core`'s `Repo`.
const MODEL: &str = "commons t.model

uses t.core

type Run = { repo: Repo }

fn repo_of(r: Run) -> Repo { r.repo }
";

/// `t.app` uses both, and declares a `Repo` of its own (#1824's shape).
const SHADOWING_APP: &str = "commons t.app

uses t.core
uses t.model

type Repo = Int

fn own(x: Repo) -> Repo { x }

fn reached(r: Run) -> Bool { r.repo.length() > 0 }
";

/// `t.web` uses only `t.model`, so `Repo` is reachable but not nameable
/// (#1807's shape).
const WEB: &str = "commons t.web

uses t.model

fn make() -> Run { Run { repo: \"x\" } }

fn read(r: Run) -> Int { r.repo.length() }
";

const GEO: &str = "commons geo\n\ntype Point = { x: Int, y: Int }\n";

/// `left` uses `geo` and so rebrands `Point` (#1704's shape).
const LEFT: &str = "context left

uses geo

service api {
  on call(p: Point) -> Effect[Point] {
    Effect.pure(Point { x: p.x + 1, y: p.y })
  }
}
";

/// A context exporting a generic type (#1736's shape).
const PAYMENT: &str = "context demo.payment

exports transparent { Page }

type Page[T] = { items: List[T] }

service list {
  on call() -> Effect[Page[Int]] {
    Effect.pure(Page { items: [1] })
  }
}
";

const CHECKOUT: &str = "context demo.checkout

consumes demo.payment

fn size(p: Page[Int]) -> Int { p.items.length() }

service count {
  on call() -> Effect[Int] {
    let p <- demo.payment.list()
    Effect.pure(size(p))
  }
}
";

const STRING_REPO: NamedKind = NamedKind::Refined(BaseType::String);

/// Rows the checker gets right today.
const ROWS: &[Row] = &[
    Row {
        name: "a local type shadows a used one in its own declarations",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            ("t/app.bynk", SHADOWING_APP),
        ],
        expect: &[
            Expect::Resolves {
                file: "t/app.bynk",
                at: "Repo) -> Repo { x }",
                unit: "t.app",
            },
            Expect::Resolves {
                file: "t/app.bynk",
                at: "Run) -> Bool",
                unit: "t.model",
            },
        ],
    },
    Row {
        name: "a commons resolves its own uses, whoever imports it",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            ("t/app.bynk", SHADOWING_APP),
        ],
        expect: &[
            Expect::Resolves {
                file: "t/model.bynk",
                at: "Repo { r.repo }",
                unit: "t.core",
            },
            Expect::Kind {
                file: "t/model.bynk",
                at: "r.repo",
                kind: STRING_REPO,
            },
        ],
    },
    Row {
        name: "a type reached two uses deep keeps its declaring unit's type",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            ("t/web.bynk", WEB),
        ],
        expect: &[
            Expect::Clean,
            Expect::Resolves {
                file: "t/web.bynk",
                at: "Run { repo",
                unit: "t.model",
            },
            Expect::Type {
                file: "t/web.bynk",
                at: "\"x\"",
                ty: "Repo",
            },
            Expect::Kind {
                file: "t/web.bynk",
                at: "\"x\"",
                kind: STRING_REPO,
            },
            Expect::Kind {
                file: "t/web.bynk",
                at: "r.repo",
                kind: STRING_REPO,
            },
        ],
    },
    Row {
        name: "a value of the wrong base for a type reached two uses deep is rejected",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            (
                "t/web.bynk",
                "commons t.web\n\nuses t.model\n\nfn make() -> Run { Run { repo: 42 } }\n",
            ),
        ],
        expect: &[Expect::Reports("bynk.types.field_value_mismatch")],
    },
    Row {
        name: "a suite resolves its target's reached record",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            ("t/web.bynk", WEB),
            (
                "tests/web.bynk",
                "suite t.web\n\ncase \"reads\" {\n  let r = Run { repo: \"x\" }\n  expect r.repo.length() == 1\n}\n",
            ),
        ],
        expect: &[
            Expect::Clean,
            Expect::Resolves {
                file: "tests/web.bynk",
                at: "Run { repo",
                unit: "t.model",
            },
        ],
    },
    // #1814: a suite's view is closed over reached types like its target's.
    Row {
        name: "a suite rejects the wrong base for a type reached two uses deep",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            ("t/web.bynk", WEB),
            (
                "tests/web.bynk",
                "suite t.web\n\ncase \"reads\" {\n  let r = Run { repo: 42 }\n  expect r.repo == r.repo\n}\n",
            ),
        ],
        expect: &[Expect::Reports("bynk.types.field_value_mismatch")],
    },
    Row {
        name: "a suite reads a reached field as its reached type",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            ("t/web.bynk", WEB),
            (
                "tests/web.bynk",
                "suite t.web\n\ncase \"reads\" {\n  let r = Run { repo: \"x\" }\n  let n: Int = r.repo\n  expect n == 1\n}\n",
            ),
        ],
        expect: &[Expect::Reports("bynk.types.let_annotation_mismatch")],
    },
    Row {
        name: "a suite may not name a type its target only reaches",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            ("t/web.bynk", WEB),
            (
                "tests/web.bynk",
                "suite t.web\n\ncase \"names\" {\n  let r = Run { repo: \"x\" }\n  let n: Repo = r.repo\n  expect n == r.repo\n}\n",
            ),
        ],
        expect: &[Expect::Reports("bynk.resolve.unknown_type")],
    },
    // Review of #1814: a reached name a suite's view binds to a different
    // declaration, where no unit's closure sees the pair.
    Row {
        name: "a system suite's uses and a participant may not bind a reached name twice",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            (
                "t/web.bynk",
                "context t.web\n\ntype Repo = Int\n\nservice api from http {\n  on GET(\"/ping\") () -> Effect[HttpResult[Repo]] by Visitor {\n    Ok(1)\n  }\n}\n",
            ),
            (
                "tests/web.bynk",
                "suite t.web as system {\n  uses t.model\n\n  case \"c\" {\n    let r = Run { repo: 5 }\n    let p <- api.GET(\"/ping\")\n    expect p is Ok(_)\n  }\n}\n",
            ),
        ],
        expect: &[Expect::Reports("bynk.uses.name_conflict")],
    },
    Row {
        name: "a consumed context's unexported type may not bind a name a suite reaches",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            (
                "t/vault.bynk",
                "context t.vault\n\nexports transparent { Box }\n\ntype Repo = Int\n\ntype Box = { n: Int }\n\nservice open {\n  on call() -> Effect[Box] {\n    Effect.pure(Box { n: 1 })\n  }\n}\n",
            ),
            (
                "t/web.bynk",
                "context t.web\n\nuses t.model\nconsumes t.vault as Vault\n\nservice ping {\n  on call() -> Effect[Int] {\n    let b <- Vault.open()\n    Effect.pure(b.n)\n  }\n}\n",
            ),
            (
                "tests/web.bynk",
                "suite t.web\n\ncase \"c\" {\n  let r = Run { repo: 5 }\n  expect r.repo == r.repo\n}\n",
            ),
        ],
        expect: &[Expect::Reports("bynk.uses.name_conflict")],
    },
    Row {
        name: "a context's rebranded uses type resolves to the commons",
        files: &[("geo.bynk", GEO), ("left.bynk", LEFT)],
        expect: &[
            Expect::Clean,
            Expect::Resolves {
                file: "left.bynk",
                at: "Point) ->",
                unit: "geo",
            },
            Expect::Resolves {
                file: "left.bynk",
                at: "Point { x",
                unit: "geo",
            },
            Expect::Type {
                file: "left.bynk",
                at: "p.x + 1",
                ty: "Int",
            },
        ],
    },
    Row {
        name: "a consumed context's generic type resolves to the consumed context",
        files: &[
            ("demo/payment.bynk", PAYMENT),
            ("demo/checkout.bynk", CHECKOUT),
        ],
        expect: &[
            Expect::Clean,
            Expect::Resolves {
                file: "demo/checkout.bynk",
                at: "Page[Int]) ->",
                unit: "demo.payment",
            },
            Expect::Type {
                file: "demo/checkout.bynk",
                at: "p.items",
                ty: "List[Int]",
            },
        ],
    },
];

#[test]
fn every_row_resolves_as_expected() {
    for row in ROWS {
        check(row);
    }
}

#[test]
fn row_names_are_unique() {
    let mut names: Vec<&str> = ROWS.iter().map(|r| r.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), ROWS.len());
}

// --- Known defects. Each fails today; when its issue is fixed, move the row
// --- into `ROWS` and delete the test.

/// #1824: `t.app`'s own `type Repo = Int` retypes the `Repo` that `t.model`'s
/// `Run` reaches, so `r.repo` is an `Int` in `t.app` and `.length()` on it is
/// rejected. It must stay `t.core`'s `String`-refined `Repo`.
#[test]
#[should_panic(
    expected = "row `an imported declaration keeps its own types under a shadowing local`"
)]
fn known_defect_1824_shadowing_local_retypes_an_imported_declaration() {
    check(&Row {
        name: "an imported declaration keeps its own types under a shadowing local",
        files: &[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            ("t/app.bynk", SHADOWING_APP),
        ],
        expect: &[
            Expect::Clean,
            Expect::Kind {
                file: "t/app.bynk",
                at: "r.repo",
                kind: STRING_REPO,
            },
        ],
    });
}

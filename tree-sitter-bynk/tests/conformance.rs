//! Cross-parser conformance: the missing guard identified in #635.
//!
//! Bynk has **two** parsers. The compiler parses with the hand-written
//! recursive-descent parser in `bynk-syntax`; the editor tooling (and the
//! normative grammar appendix rendered from it) parses with the independent
//! `tree-sitter-bynk` grammar. Every other grammar guard is intra-side —
//! appendix vs `grammar.json`, `grammar.json` vs `{{#grammar}}` directives,
//! `keywords.rs` vs `lexer.rs` — so nothing tied tree-sitter to the compiler
//! parser, and three drifts (a missing `Bytes`, unenforced variant
//! capitalisation, and over-generated built-in generic arity) lived exactly in
//! that gap.
//!
//! This test closes it: each case is parsed by **both** parsers, and their
//! accept/reject decisions must agree with each other and with the pinned
//! expectation. A drift on either side — the grammar admitting what the
//! compiler rejects, or vice versa — fails here.
//!
//! Scope is the type surface where the drifts lived (base types, sum/enum
//! variants, built-in generics). Add a case here whenever a change could move
//! the two parsers apart.
//!
//! T0.4′ widens this from cases to **totality**: every file the compiler's
//! parser accepts — `examples/`, the vendored first-party `.bynk` sources
//! (`bynk-std/lib`), every `bynkc` positive fixture — must have
//! zero `ERROR` and zero `MISSING` nodes under tree-sitter too, and every
//! negative fixture whose `expected_error.txt` names a parse/lex-time
//! category must be rejected by both. This corpus is compiled and checked
//! elsewhere for *semantics*; here only *parsing* is at stake, so each file
//! is parsed standalone rather than through the full project pipeline.

use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::Parser;

/// Does the `tree-sitter-bynk` grammar accept `src` cleanly — no `ERROR` and no
/// `MISSING` nodes? `has_error()` covers error nodes, but a recovery that
/// inserts a *missing* node (rather than an error one) is still a reject, and
/// treating it as such keeps the guard robust across tree-sitter versions as
/// cases grow.
fn tree_sitter_accepts(src: &str) -> bool {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bynk::LANGUAGE.into())
        .expect("load tree-sitter-bynk language");
    let tree = parser.parse(src, None).expect("tree-sitter parse");
    let root = tree.root_node();
    !root.has_error() && !has_missing(root)
}

/// Is `node` — or any node beneath it — a `MISSING` node?
fn has_missing(node: tree_sitter::Node) -> bool {
    if node.is_missing() {
        return true;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor).any(has_missing)
}

/// Does the compiler's own parser (`bynk-syntax`) accept `src`? Uses
/// `parse_units`, not `parse`, so a `context` file (every real project source
/// outside a bare `commons` fragment) and an atomic `commons` + `suite` file
/// (DECISION S, v0.113) are accepted rather than rejected with
/// `bynk.parse.unexpected_context` or `bynk.parse.extra_tokens` — `parse`
/// exists for the single-file CLI path, which only ever sees one bare
/// `commons` unit.
fn bynk_syntax_accepts(src: &str) -> bool {
    match bynk_syntax::lexer::tokenize(src) {
        Ok(tokens) => bynk_syntax::parser::parse_units(&tokens, src).is_ok(),
        Err(_) => false,
    }
}

/// Wrap a declaration body in a minimal `commons` unit both parsers accept.
fn unit(body: &str) -> String {
    format!("commons conf\n{body}\n")
}

struct Case {
    /// Human label, shown on failure.
    what: &'static str,
    /// A declaration body, wrapped into a `commons` unit before parsing.
    body: &'static str,
    /// The decision both parsers must reach: `true` = accept, `false` = reject.
    accept: bool,
}

const CASES: &[Case] = &[
    // -- Base types, incl. `Bytes` (drift A: the parser accepts it; the grammar
    //    omitted it, so tree-sitter could not parse `type Blob = Bytes` at all). --
    Case {
        what: "base Int",
        body: "type T = Int",
        accept: true,
    },
    Case {
        what: "base String",
        body: "type T = String",
        accept: true,
    },
    Case {
        what: "base Bool",
        body: "type T = Bool",
        accept: true,
    },
    Case {
        what: "base Float",
        body: "type T = Float",
        accept: true,
    },
    Case {
        what: "base Duration",
        body: "type T = Duration",
        accept: true,
    },
    Case {
        what: "base Instant",
        body: "type T = Instant",
        accept: true,
    },
    Case {
        what: "base Bytes",
        body: "type T = Bytes",
        accept: true,
    },
    Case {
        what: "Bytes in a generic",
        body: "fn f(s: String) -> Option[Bytes] { none }",
        accept: true,
    },
    // -- Sum / enum variant capitalisation (drift B: the parser accepted a
    //    lowercase variant end-to-end; the grammar's `constant_name` rejects it). --
    Case {
        what: "pipe variant capitalised",
        body: "type S = | Active | Inactive",
        accept: true,
    },
    Case {
        what: "pipe variant lowercase",
        body: "type S = | active | Inactive",
        accept: false,
    },
    Case {
        what: "enum tag capitalised",
        body: "type S = enum { Active, Inactive }",
        accept: true,
    },
    Case {
        what: "enum tag lowercase",
        body: "type S = enum { active, Inactive }",
        accept: false,
    },
    // The `embeds … as V` target is a `constant_name` too, and is the third
    // parse-time call-site of the capitalisation rule — cover it directly.
    Case {
        what: "embeds target capitalised",
        body: "type E = | Wrapped(reason: Int) embeds Int as Wrapped",
        accept: true,
    },
    Case {
        what: "embeds target lowercase",
        body: "type E = | Wrapped(reason: Int) embeds Int as wrapped",
        accept: false,
    },
    // -- Built-in generic arity (drift C: the grammar over-generated with
    //    `sep1`; the parser rejects the wrong arity at parse time). --
    Case {
        what: "Option[T] arity 1",
        body: "fn f(x: Option[Int]) -> Int { 0 }",
        accept: true,
    },
    Case {
        what: "Option arity 2 rejected",
        body: "fn f(x: Option[Int, String]) -> Int { 0 }",
        accept: false,
    },
    Case {
        what: "List[T] arity 1",
        body: "fn f(x: List[Int]) -> Int { 0 }",
        accept: true,
    },
    Case {
        what: "Result[T, E] arity 2",
        body: "fn f(x: Result[Int, String]) -> Int { 0 }",
        accept: true,
    },
    Case {
        what: "Result arity 1 rejected",
        body: "fn f(x: Result[Int]) -> Int { 0 }",
        accept: false,
    },
    Case {
        what: "Map[K, V] arity 2",
        body: "fn f(x: Map[String, Int]) -> Int { 0 }",
        accept: true,
    },
    Case {
        what: "Map arity 1 rejected",
        body: "fn f(x: Map[String]) -> Int { 0 }",
        accept: false,
    },
    // -- #472 refined patterns: `refined_pattern` is admitted only at a match
    //    arm's top-level pattern (and, syntactically, `is`'s RHS — rejected by
    //    the checker, not the grammar), never through a nested payload
    //    position — see D4 in design/decisions/*-refined-patterns.md. A PR
    //    review caught a drift here: `where`-suffix parsing had leaked into
    //    every recursive `parse_pattern` call, so the compiler accepted (and
    //    correctly compiled) `Ok(_ where P)` / `r is Ok(_ where P)`, which
    //    tree-sitter rejects. --
    Case {
        what: "refined pattern at match-arm top level",
        body: "fn f(x: Int) -> Int { match x { _ where NonNegative => 1 _ => 0 } }",
        accept: true,
    },
    Case {
        what: "refined pattern on is's top level rejected",
        body: "fn f(x: Int) -> Bool { x is _ where NonNegative }",
        accept: false,
    },
    Case {
        what: "refined pattern nested in a match-arm payload rejected",
        body: "fn f(r: Result[Int, String]) -> Int { match r { Ok(_ where NonNegative) => 1 Ok(_) => 2 Err(_) => 3 } }",
        accept: false,
    },
    Case {
        what: "refined pattern nested under is rejected",
        body: "fn f(r: Result[Int, String]) -> Bool { r is Ok(_ where NonNegative) }",
        accept: false,
    },
    // -- Constrained generics slice 0: bounds on type parameters and a method's
    //    `where` clause. The constraint *names* are the checker's concern, so
    //    both parsers take any identifier after the `:`. --
    Case {
        what: "fn type parameter bound",
        body: "fn same[T: Eq](a: T, b: T) -> Bool { a == b }",
        accept: true,
    },
    Case {
        what: "bounds joined with `+`",
        body: "fn f[T: Eq + Ord, U](a: T, b: U) -> T { a }",
        accept: true,
    },
    Case {
        what: "bound on a type declaration",
        body: "type P[T: Eq] = { a: T }",
        accept: true,
    },
    Case {
        what: "method where clause",
        body: "type Box[A] = { value: A }\nfn Box.has(self, x: A) -> Bool where A: Eq { self.value == x }",
        accept: true,
    },
    Case {
        what: "where clause with several entries and bounds",
        body: "type Pair[A, B] = { a: A, b: B }\nfn Pair.f(self) -> Int where A: Eq + Ord, B: Eq { 0 }",
        accept: true,
    },
    Case {
        what: "where clause before a contract clause",
        body: "type Box[A] = { value: A }\nfn Box.has(self, x: A) -> Bool where A: Eq\n  requires p: true\n{ self.value == x }",
        accept: true,
    },
    Case {
        what: "bound with a dangling `+` rejected",
        body: "fn f[T: Eq +](x: T) -> T { x }",
        accept: false,
    },
    Case {
        what: "bound with no constraint rejected",
        body: "fn f[T:](x: T) -> T { x }",
        accept: false,
    },
    Case {
        what: "empty where clause rejected",
        body: "fn f(x: Int) -> Int where { x }",
        accept: false,
    },
    Case {
        what: "where clause after a contract clause rejected",
        body: "fn f(x: Int) -> Int requires p: true where T: Eq { x }",
        accept: false,
    },
    // -- #2640: the `par { … }` binding block. `par` is contextual, so it opens
    //    a statement only before `{` and is an identifier everywhere else. Both
    //    parsers admit any statement but a `let … else` inside the braces (the
    //    checker names the forbidden ones); anything that is not a statement is
    //    refused by both. --
    Case {
        what: "par block of effectful lets and a do",
        body: "fn f(id: Int) -> Effect[Int] { par { let a <- A.get(id) let b: Int <- B.get(id) do C.touch(id) } Effect.pure(a + b) }",
        accept: true,
    },
    Case {
        what: "par block holding a statement the checker refuses",
        body: "fn f(id: Int) -> Effect[Int] { par { let a = 1 ~> N.ping(id) par { do C.touch(id) } } Effect.pure(1) }",
        accept: true,
    },
    Case {
        what: "empty par block",
        body: "fn f() -> Effect[Int] { par { } Effect.pure(1) }",
        accept: true,
    },
    Case {
        what: "par as an identifier",
        body: "fn f(par: Int) -> Int { let x = par + 1\n let y = par\n x + y }",
        accept: true,
    },
    Case {
        what: "par as a binding name before a statement",
        body: "fn f() -> Int { let par = 1\n par }",
        accept: true,
    },
    Case {
        what: "par block holding an expression rejected",
        body: "fn f() -> Effect[Int] { par { load() } Effect.pure(1) }",
        accept: false,
    },
    Case {
        what: "par block holding a let-else rejected",
        body: "fn f() -> Effect[Int] { par { let Ok(v) <- load() else { Effect.pure(0) } } Effect.pure(1) }",
        accept: false,
    },
    Case {
        what: "par block without a closing brace rejected",
        body: "fn f() -> Effect[Int] { par { let a <- A.get(1) Effect.pure(1) }",
        accept: false,
    },
    // -- #2641 record patterns (patterns track, slice 1): open, with a mandatory
    //    trailing `..`, parenthesised after `is` (where the `{` would also open
    //    the `if` block). --
    Case {
        what: "record pattern at a match-arm top level",
        body: "type O = { a: Int, b: Int }\nfn f(o: O) -> Int { match o { O { a, b: 1, .. } => a O { .. } => 0 } }",
        accept: true,
    },
    Case {
        what: "record pattern nested in a variant payload",
        body: "type O = { a: Int }\nfn f(o: Option[O]) -> Int { match o { Some(O { a, .. }) => a None => 0 } }",
        accept: true,
    },
    Case {
        what: "record pattern as an or-alternative in a match arm",
        body: "type O = { a: Int }\nfn f(o: Option[O]) -> Int { match o { Some(O { a: 1, .. }) | Some(O { a: 2, .. }) => 1 _ => 0 } }",
        accept: true,
    },
    Case {
        what: "record pattern nested in a record pattern",
        body: "type I = { x: Int }\ntype O = { i: I }\nfn f(o: O) -> Int { match o { O { i: I { x, .. }, .. } => x } }",
        accept: true,
    },
    Case {
        what: "record pattern with a trailing comma before ..",
        body: "type O = { a: Int }\nfn f(o: O) -> Int { match o { O { a, .. } => a } }",
        accept: true,
    },
    Case {
        what: "record pattern without the trailing ..",
        body: "type O = { a: Int }\nfn f(o: O) -> Int { match o { O { a } => a } }",
        accept: false,
    },
    Case {
        what: "record pattern with an empty body and no ..",
        body: "type O = { a: Int }\nfn f(o: O) -> Int { match o { O { } => 0 } }",
        accept: false,
    },
    Case {
        what: "parenthesised record pattern after is, before the if block",
        body: "type O = { a: Int }\nfn f(o: O) -> Bool { if o is (O { a: 1, .. }) { true } else { false } }",
        accept: true,
    },
    Case {
        what: "record pattern nested under Some after is",
        body: "type O = { a: Int }\nfn f(o: Option[O]) -> Bool { if o is Some(O { a: 1, .. }) { true } else { false } }",
        accept: true,
    },
    Case {
        what: "unparenthesised record pattern after is rejected",
        body: "type O = { a: Int }\nfn f(o: O) -> Bool { if o is O { a: 1, .. } { true } else { false } }",
        accept: false,
    },
    Case {
        what: "unparenthesised record pattern as an is or-alternative rejected",
        body: "type O = { a: Int }\nfn f(o: Option[O]) -> Bool { if o is None | O { a: 1, .. } { true } else { false } }",
        accept: false,
    },
    Case {
        what: "a unit variant followed by the if block still parses after is",
        body: "type S = enum { A, B }\nfn f(s: S) -> Bool { if s is A { true } else { false } }",
        accept: true,
    },
    // -- #2680 list patterns (patterns track, slice 4): the rest marker is last,
    //    and takes a name or nothing; a literal after it is reserved for range
    //    patterns. --
    Case {
        what: "empty list pattern",
        body: "fn f(xs: List[Int]) -> Int { match xs { [] => 0 _ => 1 } }",
        accept: true,
    },
    Case {
        what: "fixed-length list pattern",
        body: "fn f(xs: List[Int]) -> Int { match xs { [a, b] => a + b _ => 0 } }",
        accept: true,
    },
    Case {
        what: "list pattern with an unnamed rest",
        body: "fn f(xs: List[Int]) -> Int { match xs { [a, ..] => a\n [] => 0 } }",
        accept: true,
    },
    Case {
        what: "list pattern with a named rest",
        body: "fn f(xs: List[Int]) -> Int { match xs { [a, ..rest] => rest.length() [] => 0 } }",
        accept: true,
    },
    Case {
        what: "list pattern of only a rest",
        body: "fn f(xs: List[Int]) -> Int { match xs { [..rest] => rest.length() } }",
        accept: true,
    },
    Case {
        what: "list pattern with trailing commas",
        body: "fn f(xs: List[Int]) -> Int { match xs { [a,] => a,\n [a, ..,] => a,\n [a, ..r,] => a,\n _ => 0 } }",
        accept: true,
    },
    Case {
        what: "nested list patterns and a variant element",
        body: "fn f(xs: List[List[Option[Int]]]) -> Int { match xs { [[Some(a), ..], ..] => a _ => 0 } }",
        accept: true,
    },
    Case {
        what: "list pattern inside a variant payload",
        body: "fn f(o: Option[List[Int]]) -> Int { match o { Some([x, ..]) => x _ => 0 } }",
        accept: true,
    },
    Case {
        what: "or-pattern of list patterns",
        body: "fn f(xs: List[Int]) -> Int { match xs { [x] | [x, ..] => x\n [] => 0 } }",
        accept: true,
    },
    Case {
        what: "list pattern after is needs no parentheses",
        body: "fn f(xs: List[Int]) -> Bool { if xs is [_, ..] { true } else { false } }",
        accept: true,
    },
    Case {
        what: "empty list pattern after is",
        body: "fn f(xs: List[Int]) -> Bool { if xs is [] { true } else { false } }",
        accept: true,
    },
    Case {
        what: "record pattern inside a list pattern after is",
        body: "type S = { n: String }\nfn f(xs: List[S]) -> Bool { if xs is [S { n: \"api\", .. }, ..] { true } else { false } }",
        accept: true,
    },
    Case {
        what: "list let with an else",
        body: "fn f(xs: List[Int]) -> Int { let [h, ..] = xs else { 0 }\n h }",
        accept: true,
    },
    Case {
        what: "list let with a named rest and an else",
        body: "fn f(xs: List[Int]) -> Int { let [h, ..t] = xs else { 0 }\n h }",
        accept: true,
    },
    Case {
        what: "effectful list let with an else",
        body: "fn f(xs: List[Int]) -> Effect[Int] { let [h, ..] <- g(xs) else { Effect.pure(0) }\n Effect.pure(h) }",
        accept: true,
    },
    Case {
        what: "rest followed by another element rejected",
        body: "fn f(xs: List[Int]) -> Int { match xs { [.., last] => last _ => 0 } }",
        accept: false,
    },
    Case {
        what: "named rest followed by another element rejected",
        body: "fn f(xs: List[Int]) -> Int { match xs { [..r, last] => last _ => 0 } }",
        accept: false,
    },
    Case {
        what: "two rests rejected",
        body: "fn f(xs: List[Int]) -> Int { match xs { [.., ..] => 0 _ => 1 } }",
        accept: false,
    },
    Case {
        what: "rest followed by a literal rejected",
        body: "fn f(xs: List[Int]) -> Int { match xs { [..9] => 0 _ => 1 } }",
        accept: false,
    },
    Case {
        what: "rest followed by a negative literal rejected",
        body: "fn f(xs: List[Int]) -> Int { match xs { [a, ..-1] => 0 _ => 1 } }",
        accept: false,
    },
    Case {
        what: "rest followed by an underscore rejected",
        body: "fn f(xs: List[Int]) -> Int { match xs { [a, .._] => 0 _ => 1 } }",
        accept: false,
    },
    // -- #2654 record destructuring in `let` (patterns track, slice 2): an
    //    `else`-less `let` for a record pattern; the compiler alone judges it
    //    irrefutable (capitalisation is invisible to the grammar), so no case
    //    here pins a refutable one. --
    Case {
        what: "irrefutable record let with no else",
        body: "type I = { a: Int, b: Int }\nfn f(i: I) -> Int { let I { a, b: n, .. } = i\n a + n }",
        accept: true,
    },
    Case {
        what: "nested record let with no else",
        body: "type J = { x: Int }\ntype I = { j: J }\nfn f(i: I) -> Int { let I { j: J { x, .. }, .. } = i\n x }",
        accept: true,
    },
    Case {
        what: "effectful record let with no else",
        body: "type I = { a: Int }\nfn f(i: I) -> Effect[Int] { let I { a, .. } <- g(i)\n a }",
        accept: true,
    },
    Case {
        what: "parenthesised record let with no else",
        body: "type I = { a: Int }\nfn f(i: I) -> Int { let (I { a, .. }) = i\n a }",
        accept: true,
    },
    Case {
        what: "record let with an else block",
        body: "type I = { a: Int }\nfn f(i: I) -> Int { let I { a: 1, .. } = i else { 0 }\n 1 }",
        accept: true,
    },
    Case {
        what: "record let without the trailing ..",
        body: "type I = { a: Int }\nfn f(i: I) -> Int { let I { a } = i\n a }",
        accept: false,
    },
    Case {
        what: "record let with a type annotation",
        body: "type I = { a: Int }\nfn f(i: I) -> Int { let I { a, .. }: I = i\n a }",
        accept: false,
    },
    Case {
        what: "variant let with no else is still rejected",
        body: "fn f(r: Result[Int, String]) -> Int { let Ok(n) = r\n n }",
        accept: false,
    },
    Case {
        what: "parenthesised variant let with no else is still rejected",
        body: "fn f(r: Result[Int, String]) -> Int { let (Ok(n)) = r\n n }",
        accept: false,
    },
    Case {
        what: "a plain let of a capitalised name is still a binding",
        body: "fn f(i: Int) -> Int { let Total = i\n Total }",
        accept: true,
    },
];

#[test]
fn tree_sitter_and_compiler_parser_agree() {
    let mut mismatches = Vec::new();
    for case in CASES {
        let src = unit(case.body);
        let ts = tree_sitter_accepts(&src);
        let rust = bynk_syntax_accepts(&src);

        // The load-bearing assertion: the two parsers must never disagree.
        if ts != rust {
            mismatches.push(format!(
                "  [{}] parsers DISAGREE: tree-sitter accepts={ts}, bynk-syntax accepts={rust}\n      src: {:?}",
                case.what, src
            ));
            continue;
        }
        // And their shared decision must match the pinned expectation, so a
        // regression that flips both in lockstep is still caught.
        if ts != case.accept {
            mismatches.push(format!(
                "  [{}] both parsers reached accept={ts}, expected accept={}\n      src: {:?}",
                case.what, case.accept, src
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "tree-sitter and the compiler parser diverged (#635 guard):\n{}",
        mismatches.join("\n")
    );
}

/// The workspace root — this crate lives one directory below it.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tree-sitter-bynk has a parent directory")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

/// Recursively collect every `.bynk` file under `dir`. Silent on a missing
/// `dir` — callers that need "the corpus root still exists" as a totality
/// signal (rather than a quietly shrinking corpus) assert on the per-root
/// count themselves, not just the aggregate.
fn collect_bynk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_bynk_files(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("bynk") {
            out.push(p);
        }
    }
}

/// Collect every `.bynk` file under `root`, asserting the root itself
/// contributed at least one — a renamed or moved directory must fail loudly
/// here rather than silently shrink the corpus this test claims to cover.
fn collect_bynk_files_from(root: &Path, out: &mut Vec<PathBuf>) {
    let before = out.len();
    collect_bynk_files(root, out);
    assert!(
        out.len() > before,
        "no `.bynk` files found under {} — has it moved?",
        root.display()
    );
}

#[test]
fn totality_positive_corpus_accepted_by_both_parsers() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_bynk_files_from(&root.join("examples"), &mut files);
    collect_bynk_files_from(&root.join("bynk-std/lib"), &mut files);
    collect_bynk_files_from(&root.join("bynkc/tests/fixtures/positive"), &mut files);
    files.sort();

    let mut mismatches = Vec::new();
    for path in &files {
        let src = read(path);
        let ts = tree_sitter_accepts(&src);
        let rust = bynk_syntax_accepts(&src);
        if !ts || !rust {
            mismatches.push(format!(
                "  [{}] expected both parsers to accept (zero ERROR/MISSING), got tree-sitter={ts} bynk-syntax={rust}",
                path.display()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "totality (positive corpus) failed — a file the compiler accepts is not clean under tree-sitter, or vice versa:\n{}",
        mismatches.join("\n")
    );
}

/// The first token of the first non-blank, non-comment line of a negative
/// fixture's `expected_error.txt` — its diagnostic category, e.g.
/// `bynk.parse.expected_token`. The line may go on to pin the message and
/// span (`bynk.parse.expected_token expected `}` … @ 7:3`), as an exact-set
/// fixture's lines do (#1663).
fn expected_error_category(dir: &Path) -> Option<String> {
    let f = dir.join("expected_error.txt");
    if !f.exists() {
        return None;
    }
    read(&f)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| l.split_whitespace().next())
        .map(String::from)
}

/// Is `category` raised by the lexer or parser, rather than a later
/// (semantic) pass? Only these fixtures assert something both parsers can
/// observe — a type or resolution error is invisible to a bare parse.
fn is_parse_time_category(category: &str) -> bool {
    category.starts_with("bynk.parse.") || category.starts_with("bynk.lex.")
}

/// Parse/lex-time categories this totality check does not hold tree-sitter to
/// — each is a real `bynk-syntax` rule, named here (not silently skipped) so
/// dropping an entry is a visible, deliberate act. Two different reasons live
/// in this one list, and they are not the same claim:
///
/// - **inexpressible**: no context-free production can encode it (an
///   arithmetic-magnitude check, a recursive-descent stack-depth guard).
/// - **declined**: a CFG *could* express it, but the fix is disproportionate
///   to this slice (a `binary_expr` precedence restructure reachable from
///   nearly every expression in the corpus) or would add an unguarded
///   duplicate of another source of truth (`keywords.rs`). These are
///   legitimate follow-on slices, not permanent gaps.
const EXCLUDED_PARSE_TIME_CATEGORIES: &[(&str, &str)] = &[
    (
        "bynk.lex.integer_overflow",
        "inexpressible: a magnitude check on the literal's parsed value (#1657: \
         the `Int` safe-integer range), not a grammar shape",
    ),
    (
        "bynk.lex.float_literal_overflow",
        "inexpressible: an arithmetic-magnitude check on the literal's parsed \
         value, not a grammar shape",
    ),
    (
        "bynk.parse.nesting_too_deep",
        "inexpressible: bynk-syntax's MAX_NESTING_DEPTH (#713/#714) guards the \
         recursive-descent parser's own call stack; a CFG has no bounded-depth \
         form to mirror it with",
    ),
    (
        "bynk.parse.non_associative",
        "declined: `a < b < c` needs `binary_expr`'s relational/equality tiers \
         to take operands from a strictly-lower tier instead of the shared \
         `_expression` — correct in a CFG, but a precedence-chain rewrite \
         reachable from nearly every expression in the corpus",
    ),
    (
        "bynk.parse.reserved_keyword",
        "declined: expressible only via a keyword-exclusion list on \
         `identifier`, which would duplicate `keywords.rs` with no drift \
         guard tying the two together",
    ),
    (
        "bynk.parse.doc_block_contains_code",
        "inexpressible: a doc-block is one opaque external-scanner token; the \
         rule re-parses its content (code fences removed) as declarations, \
         which no production over the token stream can see (#1885)",
    ),
    (
        "bynk.lex.slash_doc",
        "declined: to the tree-sitter grammar a `///` line is a `//` line \
         comment, and an editor highlights it as one; the compiler rejects it \
         first on its line only to point a Rust or C# habit at `---` (#2203)",
    ),
    (
        "bynk.parse.multiple_docs",
        "inexpressible: both doc shapes are extras (`doc_block`, `doc_line`), \
         which no production sees, so neither their adjacency nor which \
         declaration they document is in the tree (#1888, #2203)",
    ),
    (
        "bynk.parse.empty_agent",
        "declined: a cardinality rule (at least one handler) on a body whose \
         members interleave; expressible as `repeat1` over handlers, but that \
         changes the generated parser and the grammar-derived site pages (#1662 \
         added the first fixture for this code)",
    ),
    (
        "bynk.parse.empty_capability",
        "declined: a cardinality rule (at least one operation) — `repeat1` in a \
         CFG, but a generated-parser change out of scope for #1662, which added \
         the first fixture for this code",
    ),
    (
        "bynk.parse.empty_service",
        "declined: a cardinality rule (at least one handler) — `repeat1` in a \
         CFG, but a generated-parser change out of scope for #1662, which added \
         the first fixture for this code",
    ),
    (
        "bynk.parse.expected_agent_storage",
        "declined: a cardinality rule (at least one `store` field) on a body \
         whose members interleave — expressible, but a generated-parser change \
         out of scope for #1662, which added the first fixture for this code",
    ),
    (
        "bynk.parse.self_outside_method",
        "declined: `self` is an ordinary parameter name to the tree-sitter \
         grammar; restricting it to a method's first parameter needs a separate \
         `fn_decl` production for methods (#1662 added the first fixture)",
    ),
    (
        "bynk.parse.unexpected_adapter",
        "inexpressible: not a grammar rule — the file is a well-formed adapter \
         unit, and only single-file compilation (which accepts a `commons` \
         alone) rejects it",
    ),
    (
        "bynk.parse.unexpected_context",
        "inexpressible: not a grammar rule — the file is a well-formed context \
         unit, and only single-file compilation (which accepts a `commons` \
         alone) rejects it",
    ),
    (
        "bynk.parse.unexpected_suite",
        "inexpressible: not a grammar rule — the file is a well-formed suite \
         unit, and only single-file compilation (which accepts a `commons` \
         alone) rejects it",
    ),
    (
        "bynk.parse.uses_after_decls",
        "declined: the ordering rule is fragment-form-only (`!brace` in \
         declarations.rs) and differs per unit kind (commons/context: before \
         `type`/`fn`; suite: before `stub`/`case`/`property`) — three \
         separate fragment-only item-list productions to add",
    ),
];

/// Negative fixtures whose parse-time rejection depends on something a
/// context-free grammar cannot see, named by directory so the rest of their
/// category stays checked. Dropping an entry is a visible, deliberate act.
const EXCLUDED_PARSE_TIME_FIXTURES: &[(&str, &str)] = &[(
    "2654_parse_let_record_pattern_without_else",
    "inexpressible: whether a record pattern's field is a binding (irrefutable) \
     or a variant (refutable) is a matter of capitalisation, which the \
     tree-sitter grammar's single `identifier` token cannot see; the \
     `else`-less `let` is accepted for any record pattern there (#2654)",
)];

#[test]
fn totality_parse_error_negative_fixtures_rejected_by_both_parsers() {
    let root = workspace_root();
    let negative_root = root.join("bynkc/tests/fixtures/negative");
    let mut dirs = Vec::new();
    if let Ok(rd) = fs::read_dir(&negative_root) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                dirs.push(p);
            }
        }
    }
    dirs.sort();
    assert!(!dirs.is_empty(), "no negative fixtures found");

    let mut checked = 0;
    let mut excluded_counts: Vec<(&str, usize)> = EXCLUDED_PARSE_TIME_CATEGORIES
        .iter()
        .map(|(cat, _)| (*cat, 0))
        .collect();
    let mut mismatches = Vec::new();
    for dir in &dirs {
        let Some(category) = expected_error_category(dir) else {
            continue;
        };
        if !is_parse_time_category(&category) {
            continue;
        }
        if let Some(entry) = excluded_counts.iter_mut().find(|(cat, _)| *cat == category) {
            entry.1 += 1;
            continue;
        }
        if EXCLUDED_PARSE_TIME_FIXTURES
            .iter()
            .any(|(name, _)| dir.file_name().is_some_and(|n| n == *name))
        {
            continue;
        }

        let mut files = Vec::new();
        collect_bynk_files(dir, &mut files);
        files.sort();

        // A fixture's `src/` may hold sibling files that are unrelated and
        // parse cleanly; the one(s) `bynk-syntax` itself rejects are the ones
        // both parsers must agree are rejected.
        let rejecting: Vec<&PathBuf> = files
            .iter()
            .filter(|f| !bynk_syntax_accepts(&read(f)))
            .collect();
        if rejecting.is_empty() {
            mismatches.push(format!(
                "  [{}] expected_error.txt names a parse/lex category ({category}) but bynk-syntax accepted every file in the fixture",
                dir.display()
            ));
            continue;
        }
        checked += 1;
        for f in rejecting {
            let src = read(f);
            if tree_sitter_accepts(&src) {
                mismatches.push(format!(
                    "  [{}] parsers DISAGREE on {}: bynk-syntax rejects, tree-sitter accepts",
                    dir.display(),
                    f.display()
                ));
            }
        }
    }
    assert!(
        checked > 0,
        "no parse/lex-time negative fixtures were found to check"
    );
    println!(
        "totality (parse-error negative fixtures): {checked} checked, {} excluded by category",
        excluded_counts.iter().map(|(_, n)| n).sum::<usize>()
    );
    // A dead exclusion — a category no fixture raises any more — is exactly
    // the stale bookkeeping this track exists to remove (§5).
    for (cat, count) in &excluded_counts {
        assert!(
            *count > 0,
            "EXCLUDED_PARSE_TIME_CATEGORIES names `{cat}`, but no negative \
             fixture raises it any more — remove the stale exclusion"
        );
    }
    assert!(
        mismatches.is_empty(),
        "totality (parse-error negative fixtures) failed:\n{}",
        mismatches.join("\n")
    );
}

//! 1A.23 (#3724): the persisted-store calls (held `Map`, `Map`, `Set`, `Cache`, `Log`, `Cell`)
//! lower to nodes. Each source below is emitted as a one-context project and every handler's
//! TypeScript is pinned in `store_nodes_cases.txt`: an operation over identifier arguments is a
//! node (one statement per line, the printer's layout), and the same operation over an argument
//! that is still text is the one-line text it printed before. The text-exit counter is read where
//! it is real (the driver links a non-test copy of `bynk-emit`, see `text_exit_counters.rs`).
use bynk_check::project_model::BuildTarget;
use bynk_emit::emitter::text_counters;

const AGENTS: &str = "context commerce

capability Clock {
  fn now() -> Effect[Instant]
}

provides Clock = SystemClock {
  fn now() -> Effect[Instant] {
    Instant.fromEpochMillis(0)
  }
}

type Item = { name: String, qty: Int }
type Hit = { sku: String, qty: Int }

agent Shelf {
  key id: String
  store items: Map[String, Item]
  store tags: Set[String]
  store seen: Cache[String, Int] @ttl(5.minutes)
  store history: Log[Hit] @retain(30.days)
  store trail: Log[Hit]
  store n: Cell[Int] = 0

  on call mapPut(k: String, v: Item) -> Effect[()] { let _ <- items.put(k, v)\n Effect.pure(()) }
  on call mapRemove(k: String) -> Effect[()] { let _ <- items.remove(k)\n Effect.pure(()) }
  on call mapContains(k: String) -> Effect[Bool] { items.contains(k) }
  on call mapSize() -> Effect[Int] { items.size() }
  on call mapGet(k: String) -> Effect[Option[Item]] { items.get(k) }
  on call mapUpdate(k: String) -> Effect[()] { let _ <- items.update(k, (i) => Item { ...i, qty: i.qty + 1 })\n Effect.pure(()) }
  on call mapUpsert(k: String, d: Item) -> Effect[()] { let _ <- items.upsert(k, d, (i) => Item { ...i, qty: i.qty + 1 })\n Effect.pure(()) }
  on call mapCount() -> Effect[Int] { items.filter((i) => i.qty > 1).count() }
  on call setAdd(k: String) -> Effect[()] { let _ <- tags.add(k)\n Effect.pure(()) }
  on call setRemove(k: String) -> Effect[()] { let _ <- tags.remove(k)\n Effect.pure(()) }
  on call setContains(k: String) -> Effect[Bool] { tags.contains(k) }
  on call setSize() -> Effect[Int] { tags.size() }
  on call cachePut(k: String, v: Int) -> Effect[()] given Clock { let _ <- seen.put(k, v)\n Effect.pure(()) }
  on call cacheRemove(k: String) -> Effect[()] given Clock { let _ <- seen.remove(k)\n Effect.pure(()) }
  on call cacheGet(k: String) -> Effect[Option[Int]] given Clock { seen.get(k) }
  on call cacheContains(k: String) -> Effect[Bool] given Clock { seen.contains(k) }
  on call cacheSize() -> Effect[Int] given Clock { seen.size() }
  on call cacheUpdate(k: String) -> Effect[()] given Clock { let _ <- seen.update(k, (v) => v + 1)\n Effect.pure(()) }
  on call cacheUpsert(k: String, d: Int) -> Effect[()] given Clock { let _ <- seen.upsert(k, d, (v) => v + 1)\n Effect.pure(()) }
  on call logAppend(h: Hit) -> Effect[()] given Clock { let _ <- history.append(h)\n Effect.pure(()) }
  on call trailAppend(h: Hit) -> Effect[()] given Clock { let _ <- trail.append(h)\n Effect.pure(()) }
  on call logRecent(c: Int) -> Effect[Int] { history.recent(c).count() }
  on call logSince(t: Instant) -> Effect[Int] { history.since(t).count() }
  on call logBefore(t: Instant) -> Effect[Int] { history.before(t).count() }
  on call logBetween(a: Instant, b: Instant) -> Effect[Int] { history.between(a, b).count() }
  on call logReversed() -> Effect[Int] { history.reversed().count() }
  on call logCount() -> Effect[Int] { history.filter((h) => h.qty > 1).count() }
  on call cellUpdate() -> Effect[()] { let _ <- n.update((c) => c + 1)\n Effect.pure(()) }
}
";

fn emit(files: &[(&str, &str)], target: BuildTarget, module: &str) -> (usize, String) {
    text_counters::reset();
    let out = match bynk_driver::project::compile_files_in_memory(
        files,
        target,
        Default::default(),
        true,
    ) {
        Ok(out) => out,
        Err(failure) => panic!(
            "fixture should compile, got:\n{}",
            failure
                .flatten()
                .iter()
                .map(|e| format!("{}: {}\n", e.category, e.message))
                .collect::<String>()
        ),
    };
    let text: String = out
        .artefacts
        .docs
        .iter()
        .filter(|(p, _)| p.to_string_lossy().ends_with(module))
        .map(|(_, d)| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    (text_counters::exits(), text)
}

const ROOM: &str = "context room

type RoomId = opaque String
type UserId = opaque String
type ServerFrame = { text: String }

agent Room {
  key id: RoomId
  store conns: Map[UserId, Connection[ServerFrame]]

  on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] { let _ <- conns.put(u, conn)\n () }
  on call rejoin(u: UserId) -> Effect[()] {
    let held <- conns.get(u)
    match held {
      Some(c) => { let _ <- conns.put(u, c)\n () }
      None => (),
    }
  }
  on call leave(u: UserId) -> Effect[()] { let _ <- conns.remove(u)\n () }
  on call has(u: UserId) -> Effect[Bool] { conns.contains(u) }
  on call count() -> Effect[Int] { conns.size() }
  on call open() -> Effect[Int] { conns.filter((c) => true).count() }
}
";

/// Only a held `Map` is traversed, so its path alone decides the settle helper's import.
const HELD: &str = "context held

type Frame = { text: String }

agent Held {
  key id: String
  store conns: Map[String, Connection[Frame]]

  on call sweep() -> Effect[()] { conns.parTraverse((c) => Effect.pure(())) }
}
";

const INDEXED: &str = "context shop

type Reservation = { id: String, orderId: String, qty: Int }

agent Inventory {
  key sku: String
  store reservations: Map[String, Reservation] @indexed(by: orderId, by: id)

  on call routed(oid: String) -> Effect[Int] { reservations.filter((r) => r.orderId == oid).count() }
  on call routedFlipped(oid: String) -> Effect[Int] { reservations.filter((r) => oid == r.orderId).count() }
  on call routedText(oid: String) -> Effect[Int] { reservations.filter((r) => r.orderId == oid.splitRegex(\",\").firstOrElse(oid)).count() }
  on call byId(rid: String) -> Effect[Int] { reservations.filter((r) => r.id == rid).count() }
  on call scan(oid: String) -> Effect[Int] { reservations.filter((r) => r.qty == 1).count() }
  on call dependent() -> Effect[Int] { reservations.filter((r) => r.orderId == r.id).count() }
}
";

const PAR: &str = "context par

type Item = { name: String, qty: Int }

agent Bag {
  key id: String
  store items: Map[String, Item]
  store n: Cell[Int] = 0
  store seen: Set[String]

  on call each() -> Effect[()] { items.parTraverse((i) => Effect.pure(())) }
  on call bump() -> Effect[()] { let _ <- n.update((c) => match c { 0 => 1, _ => c + 1 })\n Effect.pure(()) }
  on call bumpText(k: String) -> Effect[()] { let _ <- n.update((c) => c + k.splitRegex(\",\").firstOrElse(k).length())\n Effect.pure(()) }
  on call keysOf() -> Effect[List[String]] { items.keys.collect() }
  on call valuesOf() -> Effect[Int] { items.values.count() }
  on call entriesOf() -> Effect[Int] { items.entries.count() }
  on call addText(k: String) -> Effect[()] { let _ <- seen.add(k.splitRegex(\",\").firstOrElse(k))\n Effect.pure(()) }
}
";

const TEXT_ARGS: &str = "context fall

capability Clock {
  fn now() -> Effect[Instant]
}

provides Clock = SystemClock {
  fn now() -> Effect[Instant] {
    Instant.fromEpochMillis(0)
  }
}

type Hit = { sku: String, qty: Int }
type Frame = { text: String }

agent Fall {
  key id: String
  store items: Map[String, Int]
  store tags: Set[String]
  store seen: Cache[String, Int] @ttl(5.minutes)
  store history: Log[Hit] @retain(30.days)
  store n: Cell[Int] = 0
  store conns: Map[String, Connection[Frame]]

  on call mapPut(k: String) -> Effect[()] { let _ <- items.put(k.splitRegex(\",\").firstOrElse(k), 1)\n Effect.pure(()) }
  on call mapGet(k: String) -> Effect[Option[Int]] { items.get(k.splitRegex(\",\").firstOrElse(\"\")) }
  on call setAdd(k: String) -> Effect[()] { let _ <- tags.add(k.splitRegex(\",\").firstOrElse(k))\n Effect.pure(()) }
  on call cachePut(k: String) -> Effect[()] given Clock { let _ <- seen.put(k.splitRegex(\",\").firstOrElse(k), 1)\n Effect.pure(()) }
  on call logAppend(k: String) -> Effect[()] given Clock { let _ <- history.append(Hit { sku: k.splitRegex(\",\").firstOrElse(k), qty: 1 })\n Effect.pure(()) }
  on call logSince(t: Instant, k: String) -> Effect[Int] { history.since(t).filter((h) => h.sku == k.splitRegex(\",\").firstOrElse(k)).count() }
  on call cellUpdate(k: String) -> Effect[()] { let _ <- n.update((c) => c + k.splitRegex(\",\").firstOrElse(k).length())\n Effect.pure(()) }
  on call join(k: String, conn: Connection[Frame]) -> Effect[()] { let _ <- conns.put(k.splitRegex(\",\").firstOrElse(k), conn)\n () }
  on call leave(k: String) -> Effect[()] { let _ <- conns.remove(k.splitRegex(\",\").firstOrElse(k))\n () }
}
";

/// Every source, with how it is emitted: its name in the cases file, the files, the target and
/// the module the handlers land in.
fn sources() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    BuildTarget,
    &'static str,
)> {
    vec![
        (
            "agents",
            "commerce.bynk",
            AGENTS,
            BuildTarget::Bundle,
            "commerce.ts",
        ),
        (
            "room_bundle",
            "room.bynk",
            ROOM,
            BuildTarget::Bundle,
            "room.ts",
        ),
        (
            "room_workers",
            "room.bynk",
            ROOM,
            BuildTarget::Workers,
            "room/handlers.ts",
        ),
        (
            "indexed",
            "shop.bynk",
            INDEXED,
            BuildTarget::Bundle,
            "shop.ts",
        ),
        ("par_bundle", "par.bynk", PAR, BuildTarget::Bundle, "par.ts"),
        (
            "fall_bundle",
            "fall.bynk",
            TEXT_ARGS,
            BuildTarget::Bundle,
            "fall.ts",
        ),
        (
            "fall_workers",
            "fall.bynk",
            TEXT_ARGS,
            BuildTarget::Workers,
            "fall/handlers.ts",
        ),
    ]
}

/// The emitted handler `name`, from its signature to its closing brace.
fn handler<'a>(text: &'a str, name: &str) -> &'a str {
    let start = text
        .find(&format!("  async {name}("))
        .unwrap_or_else(|| panic!("`{name}` is emitted:\n{text}"));
    let len = text[start..].find("\n  }\n").expect("the handler closes");
    &text[start..start + len + "\n  }".len()]
}

const CASES: &str = include_str!("store_nodes_cases.txt");

/// `(source, handler, expected text)` for every case in the pinned file.
fn cases() -> Vec<(&'static str, &'static str, &'static str)> {
    CASES
        .strip_prefix("=== ")
        .expect("the cases file opens with a header")
        .split("\n=== ")
        .map(|c| {
            let (head, body) = c.split_once('\n').expect("a case has a header line");
            let (src, name) = head.split_once(' ').expect("`<source> <handler>`");
            (src, name, body.trim_end())
        })
        .collect()
}

#[test]
fn every_store_operation_prints_as_pinned() {
    let _text = text_counters::force_text("splitRegex");
    let mut checked = 0;
    for (src, file, text, target, module) in sources() {
        let (exits, emitted) = emit(&[(file, text)], target, module);
        for (case_src, name, want) in cases() {
            if case_src == src {
                assert_eq!(handler(&emitted, name), want, "{src}: {name}");
                checked += 1;
            }
        }
        // The identifier-argument sources are nodes from end to end; the fallback source
        // keeps text where an argument is text.
        if src.starts_with("fall") {
            assert!(exits > 0, "{src}: the text arguments exit as text");
        } else if src.starts_with("room") || src == "agents" {
            assert_eq!(exits, 0, "{src}: no store operation left as text");
        }
    }
    assert_eq!(checked, cases().len(), "every pinned case was read");
}

#[test]
fn a_text_argument_falls_back_to_the_one_line_text_byte_for_byte() {
    let _text = text_counters::force_text("splitRegex");
    // Each arm, over an argument that is still text: the one-line IIFE the arm printed before.
    let (_, bundle) = emit(&[("fall.bynk", TEXT_ARGS)], BuildTarget::Bundle, "fall.ts");
    for (name, want) in [
        (
            "mapPut",
            "const __r0 = await (async () => { await __state.items.put(String(((__xs: readonly string[], __d: string) => __xs.length > 0 ? __xs[0] : __d)(((__s: string, __r: globalThis.RegExp): readonly string[] => { if (__s.length === 0) { return __r.exec(__s) === null ? [\"\"] : []; } const __out: string[] = []; let __p = 0; let __q = 0; while (__q < __s.length) { __r.lastIndex = __q; const __hit = __r.exec(__s); const __e = __r.lastIndex; if (__hit === null || __e === __p) { __q++; } else { __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; } } __out.push(__s.slice(__p)); return __out; })(k, new globalThis.RegExp(\",\", \"y\")), k)), 1); return undefined; })();",
        ),
        (
            "setAdd",
            "const __r0 = await (async () => { await __state.tags.add(String(((__xs: readonly string[], __d: string) => __xs.length > 0 ? __xs[0] : __d)(((__s: string, __r: globalThis.RegExp): readonly string[] => { if (__s.length === 0) { return __r.exec(__s) === null ? [\"\"] : []; } const __out: string[] = []; let __p = 0; let __q = 0; while (__q < __s.length) { __r.lastIndex = __q; const __hit = __r.exec(__s); const __e = __r.lastIndex; if (__hit === null || __e === __p) { __q++; } else { __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; } } __out.push(__s.slice(__p)); return __out; })(k, new globalThis.RegExp(\",\", \"y\")), k))); return undefined; })();",
        ),
        (
            "cachePut",
            "const __r0 = await (async () => { const __now = __noteClock(__state, await deps.Clock.now()); await __state.seen.put(String(((__xs: readonly string[], __d: string) => __xs.length > 0 ? __xs[0] : __d)(((__s: string, __r: globalThis.RegExp): readonly string[] => { if (__s.length === 0) { return __r.exec(__s) === null ? [\"\"] : []; } const __out: string[] = []; let __p = 0; let __q = 0; while (__q < __s.length) { __r.lastIndex = __q; const __hit = __r.exec(__s); const __e = __r.lastIndex; if (__hit === null || __e === __p) { __q++; } else { __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; } } __out.push(__s.slice(__p)); return __out; })(k, new globalThis.RegExp(\",\", \"y\")), k)), { v: 1, exp: __now + 300000 }); return undefined; })();",
        ),
        (
            "logAppend",
            "const __r0 = await (async () => { const __now = await deps.Clock.now(); await __state.history.append({ sku: ((__xs: readonly string[], __d: string) => __xs.length > 0 ? __xs[0] : __d)(((__s: string, __r: globalThis.RegExp): readonly string[] => { if (__s.length === 0) { return __r.exec(__s) === null ? [\"\"] : []; } const __out: string[] = []; let __p = 0; let __q = 0; while (__q < __s.length) { __r.lastIndex = __q; const __hit = __r.exec(__s); const __e = __r.lastIndex; if (__hit === null || __e === __p) { __q++; } else { __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; } } __out.push(__s.slice(__p)); return __out; })(k, new globalThis.RegExp(\",\", \"y\")), k), qty: 1 }, __now); await __state.history.prune(__now - 2592000000); return undefined; })();",
        ),
        (
            "cellUpdate",
            "const __r0 = await (() => { __state.n = ((c: number) => c + (((__xs: readonly string[], __d: string) => __xs.length > 0 ? __xs[0] : __d)(((__s: string, __r: globalThis.RegExp): readonly string[] => { if (__s.length === 0) { return __r.exec(__s) === null ? [\"\"] : []; } const __out: string[] = []; let __p = 0; let __q = 0; while (__q < __s.length) { __r.lastIndex = __q; const __hit = __r.exec(__s); const __e = __r.lastIndex; if (__hit === null || __e === __p) { __q++; } else { __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; } } __out.push(__s.slice(__p)); return __out; })(k, new globalThis.RegExp(\",\", \"y\")), k)).length)(__state.n); return undefined; })();",
        ),
    ] {
        assert!(
            handler(&bundle, name).contains(want),
            "{name}:\n{}",
            handler(&bundle, name)
        );
    }
    // The held `Map` on Workers.
    let (_, workers) = emit(
        &[("fall.bynk", TEXT_ARGS)],
        BuildTarget::Workers,
        "fall/handlers.ts",
    );
    assert!(handler(&workers, "join").contains(
        "(async () => { const __k = String(((__xs: readonly string[], __d: string) => __xs.length > 0 ? __xs[0] : __d)(((__s: string, __r: globalThis.RegExp): readonly string[] => { if (__s.length === 0) { return __r.exec(__s) === null ? [\"\"] : []; } const __out: string[] = []; let __p = 0; let __q = 0; while (__q < __s.length) { __r.lastIndex = __q; const __hit = __r.exec(__s); const __e = __r.lastIndex; if (__hit === null || __e === __p) { __q++; } else { __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; } } __out.push(__s.slice(__p)); return __out; })(k, new globalThis.RegExp(\",\", \"y\")), k)); const __new = __connIdOf(conn);"
    ));
    assert!(handler(&workers, "leave").contains("(async () => { const __k = String(((__xs: readonly string[], __d: string) => __xs.length > 0 ? __xs[0] : __d)(((__s: string, __r: globalThis.RegExp): readonly string[] => { if (__s.length === 0) { return __r.exec(__s) === null ? [\"\"] : []; } const __out: string[] = []; let __p = 0; let __q = 0; while (__q < __s.length) { __r.lastIndex = __q; const __hit = __r.exec(__s); const __e = __r.lastIndex; if (__hit === null || __e === __p) { __q++; } else { __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; } } __out.push(__s.slice(__p)); return __out; })(k, new globalThis.RegExp(\",\", \"y\")), k)); const __cid = await __state.conns.get(__k);"));
}

#[test]
fn a_connection_map_closes_what_it_displaces_on_both_targets() {
    // The bundle compares the connections themselves; Workers compares connection ids and
    // resolves the displaced one. Neither closes it when the same connection goes back under
    // its own key (`__old !== __new`).
    let (_, bundle) = emit(&[("room.bynk", ROOM)], BuildTarget::Bundle, "room.ts");
    let join = handler(&bundle, "join");
    assert!(join.contains(
        "if (__old !== undefined && __old !== __new) {\n          await __old.close();\n        }"
    ));
    let (_, workers) = emit(
        &[("room.bynk", ROOM)],
        BuildTarget::Workers,
        "room/handlers.ts",
    );
    let join = handler(&workers, "join");
    assert!(join.contains("const __new = __connIdOf(conn);"));
    assert!(join.contains("if (__old !== undefined && __old !== __new) {\n          const __c = __resolveConnection<ServerFrame>(this.state, __old);"));
}

#[test]
fn an_indexed_equality_filter_routes_to_the_posting_list() {
    let (_, text) = emit(&[("shop.bynk", INDEXED)], BuildTarget::Bundle, "shop.ts");
    for name in ["routed", "routedFlipped"] {
        assert!(
            handler(&text, name).contains("__state.reservations.byIndex(\"orderId\", String(oid))"),
            "{name}"
        );
    }
    assert!(handler(&text, "byId").contains("byIndex(\"id\", String(rid))"));
    // A filter that is not an equality on an indexed field, or whose value mentions the
    // parameter, scans.
    for name in ["scan", "dependent", "routedText"] {
        let h = handler(&text, name);
        assert!(!h.contains("byIndex"), "{name} scans:\n{h}");
        assert!(h.contains("__state.reservations.values()"), "{name}");
    }
}

#[test]
fn a_cache_operation_notes_the_clock_once() {
    let (_, text) = emit(
        &[("commerce.bynk", AGENTS)],
        BuildTarget::Bundle,
        "commerce.ts",
    );
    for name in [
        "cachePut",
        "cacheGet",
        "cacheContains",
        "cacheSize",
        "cacheUpdate",
        "cacheUpsert",
    ] {
        let h = handler(&text, name);
        assert_eq!(h.matches("__noteClock(").count(), 1, "{name}:\n{h}");
    }
    // `remove` reads no clock, and the expiry arithmetic is the ttl in milliseconds.
    assert_eq!(
        handler(&text, "cacheRemove")
            .matches("__noteClock(")
            .count(),
        0
    );
    for name in ["cachePut", "cacheUpdate", "cacheUpsert"] {
        assert!(
            handler(&text, name).contains("exp: __now + 300000"),
            "{name}"
        );
    }
}

#[test]
fn a_parallel_traverse_over_a_store_imports_the_settle_helper_on_both_paths() {
    for (files, target, module, helper) in [
        (
            ("par.bynk", PAR),
            BuildTarget::Bundle,
            "par.ts",
            "./__bynk_runtime/settle.js",
        ),
        (
            ("par.bynk", PAR),
            BuildTarget::Workers,
            "par/handlers.ts",
            "../../__bynk_runtime/settle.js",
        ),
        (
            ("held.bynk", HELD),
            BuildTarget::Bundle,
            "held.ts",
            "./__bynk_runtime/settle.js",
        ),
        (
            ("held.bynk", HELD),
            BuildTarget::Workers,
            "held/handlers.ts",
            "../../__bynk_runtime/settle.js",
        ),
    ] {
        let (_, text) = emit(&[files], target, module);
        assert!(
            text.contains(&format!("import {{ __bynkSettleAll }} from \"{helper}\";")),
            "{module}"
        );
    }
}

#[test]
fn a_cell_update_over_a_statement_bodied_lambda_keeps_its_block_inside_the_update() {
    let (_, text) = emit(&[("par.bynk", PAR)], BuildTarget::Bundle, "par.ts");
    let h = handler(&text, "bump");
    assert!(h.contains(
        "const __r0 = await (() => {\n        __state.n = ((c: number) => ((__d) => {\n          switch (__d) {"
    ));
    assert!(h.contains("})(c))(__state.n);\n        return undefined;\n      })();"));
}

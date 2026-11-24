//! 1A.33: the typed payloads the checker records on each [`Callee`], one test
//! per family. The strings these sit beside are what the emitter used to
//! compare; the typed forms are what it reads.

use crate::checker::{self, Callee, HandlerAddress, WsAddress, WsMessageShape};
use crate::firstparty::FirstPartyCap;
use crate::kernel_op::*;
use crate::resolver;
use crate::testkit::analyse;
use bynk_syntax::ast::HttpMethod;
use bynk_syntax::{lexer, parser};

/// Check a single `commons` unit and return every callee the checker recorded.
fn callees_for(source: &str) -> Vec<Callee> {
    let tokens = lexer::tokenize(source).expect("lex");
    let (commons, _warnings) = parser::parse_with_warnings(&tokens, source).expect("parse");
    let resolved = resolver::resolve(commons).expect("resolve");
    let typed = checker::check(resolved).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    typed.callees.into_values().collect()
}

/// Every typed static operation recorded in `source`, sorted by spelling.
fn static_ops(source: &str) -> Vec<StaticOp> {
    callees_for(source)
        .into_iter()
        .filter_map(|c| match c {
            Callee::Intrinsic { ns, op, .. } => {
                assert_eq!(op.ns(), ns, "the typed namespace follows the operation");
                Some(op)
            }
            _ => None,
        })
        .collect()
}

fn assert_has(ops: &[StaticOp], expected: StaticOp) {
    assert!(ops.contains(&expected), "{expected:?} missing from {ops:?}");
}

/// Every callee in the table below records its typed twin, and the strings agree
/// with it: the same spelling the emitter used to compare.
#[test]
fn every_value_level_static_records_its_typed_operation() {
    let ops = static_ops(
        r#"
commons demo {
  type Row = { id: Int }

  fn list() -> List[Int] { List.empty() }
  fn map() -> Map[String, Int] { Map.empty() }
  fn map_of(m: Map[String, Int]) -> Map[String, Int] { Map.of(m.entries()) }
  fn set() -> Set[Int] { Set.empty() }
  fn set_of() -> Set[Int] { Set.of([1, 2]) }
  fn int_parse() -> Option[Int] { Int.parse("1") }
  fn float_parse() -> Option[Float] { Float.parse("1") }
  fn pi() -> Float { Float.pi() }
  fn e() -> Float { Float.e() }
  fn dec_parse() -> Option[Decimal] { Decimal.parse("1") }
  fn dec_from_int() -> Decimal { Decimal.fromInt(1) }
  fn millis() -> Duration { Duration.millis(5) }
  fn epoch() -> Instant { Instant.fromEpochMillis(5) }
  fn iso() -> Option[Instant] { Instant.fromIso("2020-01-01T00:00:00Z") }
  fn utf8() -> Bytes { Bytes.fromUtf8("x") }
  fn b64() -> Option[Bytes] { Bytes.fromBase64("eA==") }
  fn hex() -> Option[Bytes] { Bytes.fromHex("78") }
  fn b64url() -> Option[Bytes] { Bytes.fromBase64Url("eA") }
  fn bytes_empty() -> Bytes { Bytes.empty() }
  fn stream() -> Stream[Int] { Stream.of([1, 2]) }
  fn encode(r: Row) -> String { Json.encode(r) }
  fn encode_plain(r: Row) -> String { Json.encodePlain(r) }
  fn decode(s: String) -> Result[Row, DecodeError] { Json.decode[Row](s) }
}
"#,
    );
    for expected in [
        StaticOp::List(ListStatic::Empty),
        StaticOp::Map(MapStatic::Empty),
        StaticOp::Map(MapStatic::Of),
        StaticOp::Set(SetStatic::Empty),
        StaticOp::Set(SetStatic::Of),
        StaticOp::Int(IntStatic::Parse),
        StaticOp::Float(FloatStatic::Parse),
        StaticOp::Float(FloatStatic::Pi),
        StaticOp::Float(FloatStatic::E),
        StaticOp::Decimal(DecimalStatic::Parse),
        StaticOp::Decimal(DecimalStatic::FromInt),
        StaticOp::Duration(DurationStatic::Millis),
        StaticOp::Instant(InstantStatic::FromEpochMillis),
        StaticOp::Instant(InstantStatic::FromIso),
        StaticOp::Bytes(BytesStatic::FromUtf8),
        StaticOp::Bytes(BytesStatic::FromBase64),
        StaticOp::Bytes(BytesStatic::FromHex),
        StaticOp::Bytes(BytesStatic::FromBase64Url),
        StaticOp::Bytes(BytesStatic::Empty),
        StaticOp::Stream(StreamStatic::Of),
        StaticOp::Json(JsonStatic::Encode),
        StaticOp::Json(JsonStatic::EncodePlain),
        StaticOp::Json(JsonStatic::Decode),
    ] {
        assert_has(&ops, expected);
    }
}

/// The codecs, the request views and the test-side values: the statics whose
/// namespaces are not plain value types.
#[test]
fn the_codec_and_request_view_statics_record_their_typed_operation() {
    let ops = static_ops(
        r#"
commons demo {
  type Row = { id: Int }

  fn sql(s: String) -> Result[Row, DecodeError] { Json.decodeSql[Row](s) }
  fn yaml(s: String) -> Result[Row, DecodeError] { Yaml.decode[Row](s) }
  fn form_decode(s: String) -> Result[Row, DecodeError] { Form.decode[Row](s) }
  fn form_encode(r: Row) -> String { Form.encode(r) }
  fn claims() -> TokenClaims { TokenClaims.of(["sub: ann"]) }
  fn headers() -> RequestHeaders { RequestHeaders.of(["a: b"]) }
  fn no_headers() -> RequestHeaders { RequestHeaders.empty() }
  fn query() -> RequestQuery { RequestQuery.of(["a=b"]) }
  fn no_query() -> RequestQuery { RequestQuery.empty() }
  fn cookies() -> RequestCookies { RequestCookies.of(["a=b"]) }
  fn no_cookies() -> RequestCookies { RequestCookies.empty() }
}
"#,
    );
    for expected in [
        StaticOp::Json(JsonStatic::DecodeSql),
        StaticOp::Yaml(YamlStatic::Decode),
        StaticOp::Form(FormStatic::Decode),
        StaticOp::Form(FormStatic::Encode),
        StaticOp::TokenClaims(TokenClaimsStatic::Of),
        StaticOp::RequestHeaders(RequestHeadersStatic::Of),
        StaticOp::RequestHeaders(RequestHeadersStatic::Empty),
        StaticOp::RequestQuery(RequestQueryStatic::Of),
        StaticOp::RequestQuery(RequestQueryStatic::Empty),
        StaticOp::RequestCookies(RequestCookiesStatic::Of),
        StaticOp::RequestCookies(RequestCookiesStatic::Empty),
    ] {
        assert_has(&ops, expected);
    }
}

/// The sum-variant namespaces carry the parser's own table entry, so a variant
/// the table does not have is not a callee.
#[test]
fn http_and_queue_result_variants_record_their_table_entry() {
    let ops = static_ops(
        r#"
commons demo {
  fn found() -> HttpResult[String] { NotFound }
  fn ok() -> HttpResult[String] { HttpResult.Ok("fine") }
}
"#,
    );
    let queue = analyse(&[(
        "demo/inbox.bynk",
        r#"context demo.inbox

service intake from queue("orders") {
  on message(item: String) -> Effect[QueueResult] {
    if item == "" { Retry("empty") } else { Ack }
  }
}
"#,
    )]);
    queue.assert_clean();
    let queue_ops: Vec<_> = queue
        .callees_in("demo.inbox")
        .iter()
        .filter_map(|c| match c {
            Callee::Intrinsic { op, .. } if op.ns() == IntrinsicNs::QueueResult => Some(op.name()),
            _ => None,
        })
        .collect();
    assert!(
        queue_ops.contains(&"Ack") && queue_ops.contains(&"Retry"),
        "{queue_ops:?}"
    );
    let names = |ns: IntrinsicNs| -> Vec<&'static str> {
        let mut v: Vec<_> = ops
            .iter()
            .filter(|o| o.ns() == ns)
            .map(|o| o.name())
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(names(IntrinsicNs::HttpResult), ["NotFound", "Ok"]);
}

/// A method the namespace does not have gets no typed operation (and so no
/// callee): the closed vocabulary is the validation.
#[test]
fn an_unknown_static_resolves_to_no_operation() {
    assert_eq!(StaticOp::resolve(IntrinsicNs::Int, "pi"), None);
    assert_eq!(
        StaticOp::resolve(IntrinsicNs::Float, "pi"),
        Some(StaticOp::Float(FloatStatic::Pi))
    );
    assert_eq!(StaticOp::resolve(IntrinsicNs::Json, "bogus"), None);
    assert_eq!(StaticOp::resolve(IntrinsicNs::Yaml, "encode"), None);
    assert_eq!(StaticOp::resolve(IntrinsicNs::HttpResult, "Bogus"), None);
    assert_eq!(StaticOp::resolve(IntrinsicNs::QueueResult, "Nack"), None);
}

/// Each namespace spelling and operation spelling round-trips through the typed form, and
/// the typed namespace is the one the string constants named.
#[test]
fn every_static_operation_round_trips_and_names_its_namespace() {
    macro_rules! each {
        ($($Enum:ident => $ns:ident, $variant:ident);* $(;)?) => {$(
            for op in $Enum::ALL {
                let typed = StaticOp::$variant(*op);
                assert_eq!(typed.ns(), IntrinsicNs::$ns);
                assert_eq!(typed.name(), op.name());
                assert_eq!(StaticOp::resolve(IntrinsicNs::$ns, op.name()), Some(typed));
            }
        )*};
    }
    each! {
        ListStatic => List, List; MapStatic => Map, Map; SetStatic => Set, Set;
        IntStatic => Int, Int; FloatStatic => Float, Float; DecimalStatic => Decimal, Decimal;
        DurationStatic => Duration, Duration; InstantStatic => Instant, Instant;
        BytesStatic => Bytes, Bytes; TokenClaimsStatic => TokenClaims, TokenClaims;
        RequestHeadersStatic => RequestHeaders, RequestHeaders;
        RequestQueryStatic => RequestQuery, RequestQuery;
        RequestCookiesStatic => RequestCookies, RequestCookies;
        StreamStatic => Stream, Stream; JsonStatic => Json, Json; YamlStatic => Yaml, Yaml;
        FormStatic => Form, Form; TestConnectionStatic => TestConnection, TestConnection;
        TestRunStatic => TestRun, TestRun;
    }
    assert_eq!(IntrinsicNs::TestRun.name(), checker::RUN_ID_NS);
    assert_eq!(IntrinsicNs::HttpResult.name(), "HttpResult");
    for v in bynk_syntax::ast::HTTP_VARIANTS {
        let op = StaticOp::resolve(IntrinsicNs::HttpResult, v.name).expect("a table entry");
        assert_eq!((op.ns(), op.name()), (IntrinsicNs::HttpResult, v.name));
    }
    for v in bynk_syntax::ast::QUEUE_VARIANTS {
        let op = StaticOp::resolve(IntrinsicNs::QueueResult, v.name).expect("a table entry");
        assert_eq!((op.ns(), op.name()), (IntrinsicNs::QueueResult, v.name));
    }
}

/// `Callee::intrinsic` takes its namespace from the operation.
#[test]
fn the_intrinsic_constructor_derives_the_namespace_from_the_operation() {
    let Callee::Intrinsic { ns, op } =
        Callee::intrinsic(StaticOp::Duration(DurationStatic::Millis))
    else {
        panic!("an intrinsic")
    };
    assert_eq!((ns.name(), op.name()), ("Duration", "millis"));
    assert_eq!(ns, IntrinsicNs::Duration);
    assert_eq!(op, StaticOp::Duration(DurationStatic::Millis));
}

// ---- store operations ------------------------------------------------------

const AGENT_STORES: &str = r#"context demo.stores

consumes bynk { Clock }

type Entry = { n: Int }

agent Holder {
  key id: String
  store cell: Cell[Int]
  store map: Map[String, Entry]
  store set: Set[String]
  store cache: Cache[String, Entry] @ttl(5.minutes)
  store log: Log[Entry]

  on call run() -> Effect[()] given Clock {
    let _ <- cell.update((n) => n + 1)
    let _ <- map.put("a", Entry { n: 1 })
    let _ <- map.get("a")
    let _ <- map.remove("a")
    let _ <- map.contains("a")
    let _ <- map.size()
    let _ <- map.update("a", (e) => e)
    let _ <- map.upsert("a", Entry { n: 1 }, (e) => e)
    let _ <- map.count()
    let _ <- set.add("a")
    let _ <- set.remove("a")
    let _ <- set.contains("a")
    let _ <- set.size()
    let _ <- cache.put("a", Entry { n: 1 })
    let _ <- cache.get("a")
    let _ <- cache.size()
    let _ <- log.append(Entry { n: 1 })
    let _ <- log.recent(5).collect()
    let _ <- log.since(Instant.fromEpochMillis(0)).count()
    let _ <- log.count()
    Effect.pure(())
  }
}
"#;

fn store_callees(source: &str) -> Vec<Callee> {
    let a = analyse(&[("demo/stores.bynk", source)]);
    a.assert_clean();
    a.callees_in("demo.stores").to_vec()
}

#[test]
fn each_store_entry_operation_records_its_typed_operation() {
    let callees = store_callees(AGENT_STORES);
    let store = |field: &str, typed: StoreEntryOp| {
        assert!(
            callees.iter().any(|c| matches!(c,
                Callee::Store { field: f, op } if f == field && *op == typed)),
            "{field}: {typed:?} missing from {callees:?}"
        );
    };
    store("cell", StoreEntryOp::Cell(CellStoreOp::Update));
    for op in MapStoreOp::ALL {
        store("map", StoreEntryOp::Map(*op));
    }
    store("set", StoreEntryOp::Set(SetStoreOp::Add));
    store("set", StoreEntryOp::Set(SetStoreOp::Remove));
    store("set", StoreEntryOp::Set(SetStoreOp::Contains));
    store("set", StoreEntryOp::Set(SetStoreOp::Size));
    store("cache", StoreEntryOp::Cache(MapStoreOp::Put));
    store("cache", StoreEntryOp::Cache(MapStoreOp::Get));
    store("cache", StoreEntryOp::Cache(MapStoreOp::Size));
    store("log", StoreEntryOp::Log(LogStoreOp::Append));
}

#[test]
fn a_lifting_store_operation_records_its_query_operation() {
    let callees = store_callees(AGENT_STORES);
    let query = |field: &str, typed: StoreQueryOp| {
        assert!(
            callees.iter().any(|c| matches!(c,
                Callee::Query { field: f, op, .. } if f == field && *op == typed)),
            "{field}: {typed:?} missing from {callees:?}"
        );
    };
    query("map", StoreQueryOp::Query(QueryOp::Count));
    query("log", StoreQueryOp::LogWindow(LogWindowOp::Recent));
    query("log", StoreQueryOp::LogWindow(LogWindowOp::Since));
    query("log", StoreQueryOp::Query(QueryOp::Count));
}

#[test]
fn an_unknown_store_operation_records_no_callee() {
    let source = AGENT_STORES.replace(
        "let _ <- cell.update((n) => n + 1)",
        "let _ <- cell.frobnicate()",
    );
    let a = analyse(&[("demo/stores.bynk", &source)]);
    a.assert_reports("bynk.store.unknown_op");
    assert!(
        !a.callees_in("demo.stores")
            .iter()
            .any(|c| matches!(c, Callee::Store { field, .. } if field == "cell")),
        "an operation the kind does not have is not a callee"
    );
}

#[test]
fn a_store_op_knows_whether_it_writes() {
    assert!(StoreEntryOp::Map(MapStoreOp::Put).mutates());
    assert!(!StoreEntryOp::Map(MapStoreOp::Get).mutates());
    assert!(StoreEntryOp::Cache(MapStoreOp::Upsert).mutates());
    assert!(StoreEntryOp::Set(SetStoreOp::Add).mutates());
    assert!(!StoreEntryOp::Set(SetStoreOp::Size).mutates());
    assert!(StoreEntryOp::Cell(CellStoreOp::Update).mutates());
    assert!(StoreEntryOp::Log(LogStoreOp::Append).mutates());
}

// ---- first-party capability identity ---------------------------------------

/// The `#926`/`#931` fixtures' own illustrative `capability Idempotency` (here also an
/// `Events`), in an adapter that is not `bynk`.
const OTHER_ADAPTER: &str = r#"adapter vendor {
  binding "./vendor.binding.ts"

  exports capability { Idempotency, Events }

  capability Idempotency {
    fn claim(key: String) -> Effect[Bool]
  }

  capability Events {
    fn emit(name: String) -> Effect[()]
  }

  provides Idempotency = MemoIdempotency
  provides Events = MemoEvents
}
"#;

const OTHER_BINDING: &str = r#"import type { Idempotency, Events } from "./vendor.js";

export class MemoIdempotency implements Idempotency {
  async claim(key: string): Promise<boolean> {
    return true;
  }
}

export class MemoEvents implements Events {
  async emit(name: string): Promise<void> {}
}
"#;

fn capability_callees(callees: &[Callee]) -> Vec<(&str, Option<FirstPartyCap>)> {
    callees
        .iter()
        .filter_map(|c| match c {
            Callee::Capability {
                cap, first_party, ..
            }
            | Callee::CrossCap {
                cap, first_party, ..
            } => Some((cap.as_str(), *first_party)),
            _ => None,
        })
        .collect()
}

#[test]
fn bynks_own_idempotency_and_events_resolve_as_first_party() {
    let a = analyse(&[(
        "shop/hooks.bynk",
        r#"context shop.hooks

consumes bynk { Idempotency, Claim, Events }

event Seen = { id: String }

service api {
  on call(id: String) -> Effect[()] by Caller given Idempotency, Events {
    let Claim.Claimed <- Idempotency.claim(id, 1.hour) else { Effect.pure(()) }
    Events.emit[Seen](Seen { id: id })
  }
}
"#,
    )]);
    a.assert_clean();
    let caps = capability_callees(a.callees_in("shop.hooks"));
    assert!(
        caps.contains(&("Idempotency", Some(FirstPartyCap::Idempotency))),
        "{caps:?}"
    );
    assert!(
        caps.contains(&("Events", Some(FirstPartyCap::Events))),
        "{caps:?}"
    );
}

/// #934: a same-named capability from another adapter stays unscoped, for the flattened
/// (local-name) path and for `Events`.
#[test]
fn a_same_named_non_first_party_capability_is_not_first_party() {
    let a = analyse(&[
        ("vendor.bynk", OTHER_ADAPTER),
        ("vendor.binding.ts", OTHER_BINDING),
        (
            "demo/orders.bynk",
            r#"context demo.orders

consumes vendor { Idempotency, Events }

service reserve {
  on call() -> Effect[Bool] by Caller given Idempotency, Events {
    let _ <- Events.emit("order")
    Idempotency.claim("order-key-1")
  }
}
"#,
        ),
    ]);
    a.assert_clean();
    let caps = capability_callees(a.callees_in("demo.orders"));
    assert!(caps.contains(&("Idempotency", None)), "{caps:?}");
    assert!(caps.contains(&("Events", None)), "{caps:?}");
    assert!(
        caps.iter().all(|(_, first_party)| first_party.is_none()),
        "no capability of another adapter is first-party: {caps:?}"
    );
}

/// The qualified (`unit.Cap.op`) path is first-party only for the `bynk` unit.
#[test]
fn a_qualified_capability_call_is_first_party_only_for_bynk() {
    let a = analyse(&[
        ("vendor.bynk", OTHER_ADAPTER),
        ("vendor.binding.ts", OTHER_BINDING),
        (
            "demo/orders.bynk",
            r#"context demo.orders

consumes vendor

service reserve {
  on call() -> Effect[Bool] by Caller given vendor.Idempotency {
    vendor.Idempotency.claim("order-key-1")
  }
}
"#,
        ),
    ]);
    a.assert_clean();
    let callees = a.callees_in("demo.orders");
    assert!(
        callees.iter().any(|c| matches!(c,
            Callee::CrossCap { unit, cap, first_party: None, .. } if unit == "vendor" && cap == "Idempotency")),
        "{callees:?}"
    );
}

#[test]
fn a_qualified_call_on_bynks_own_idempotency_is_first_party() {
    let a = analyse(&[(
        "shop/hooks.bynk",
        r#"context shop.hooks

consumes bynk

service api {
  on call(id: String) -> Effect[()] by Caller given bynk.Idempotency {
    let _ <- bynk.Idempotency.dedup[String](id)
    Effect.pure(())
  }
}
"#,
    )]);
    a.assert_clean();
    let callees = a.callees_in("shop.hooks");
    assert!(
        callees.iter().any(|c| matches!(c,
            Callee::CrossCap { unit, cap, first_party: Some(FirstPartyCap::Idempotency), .. }
                if unit == "bynk" && cap == "Idempotency")),
        "{callees:?}"
    );
}

/// A capability that is not one of bynk's first-party four has no identity, even from `bynk`.
#[test]
fn bynks_other_capabilities_have_no_first_party_identity() {
    let a = analyse(&[(
        "shop/hooks.bynk",
        r#"context shop.hooks

consumes bynk { Random }

service api {
  on call() -> Effect[Int] by Caller given Random {
    Random.int(0, 10)
  }
}
"#,
    )]);
    a.assert_clean();
    let caps = capability_callees(a.callees_in("shop.hooks"));
    assert!(caps.contains(&("Random", None)), "{caps:?}");
}

// ---- test-service addresses ------------------------------------------------

fn test_addresses(files: &[(&str, &str)], unit: &str) -> Vec<(String, HandlerAddress)> {
    let a = analyse(files);
    a.assert_clean();
    a.callees_in(unit)
        .iter()
        .filter_map(|c| match c {
            Callee::TestService {
                service, address, ..
            } => Some((format!("{service} {}", address.describe()), address.clone())),
            _ => None,
        })
        .collect()
}

const JOBS: &str = r#"context cr.jobs

type Delivery = { id: String }

service worker from cron {
  on schedule("*/5 * * * *") (at: Instant) -> Effect[Result[(), String]] {
    Ok(())
  }
}

service deliveries from queue("deliveries") {
  on message(d: Delivery) -> Effect[QueueResult] {
    Ack
  }
}

service api from http {
  on GET("/count") () -> Effect[HttpResult[Int]] by Visitor {
    Ok(1)
  }
  on POST("/items") (body: Int) -> Effect[HttpResult[Int]] by Visitor {
    Ok(body)
  }
}

service calc {
  on call(n: Int) -> Effect[Int] by Caller {
    n
  }
}
"#;

/// A case addresses each handler kind; every address is typed alongside its string.
#[test]
fn each_test_service_address_records_its_typed_form() {
    let addresses = test_addresses(
        &[
            ("cr/jobs.bynk", JOBS),
            (
                "tests/jobs.bynk",
                r#"suite cr.jobs as integration {
  case "every address" {
    let a <- worker.schedule("*/5 * * * *", Instant.fromEpochMillis(960000))
    let b <- deliveries.message(Delivery { id: "c" })
    let c <- api.GET("/count") by Visitor
    let d <- api.POST("/items", 3) by Visitor
    let e <- calc.call(21)
    expect e == 21
  }
}
"#,
            ),
        ],
        "cr.jobs",
    );
    let has = |text: &str, typed: HandlerAddress| {
        assert!(
            addresses
                .iter()
                .any(|(a, t)| a.ends_with(text) && *t == typed),
            "{text}: {typed:?} missing from {addresses:?}"
        );
    };
    has("call", HandlerAddress::Call);
    has("message", HandlerAddress::Message);
    has(
        "schedule */5 * * * *",
        HandlerAddress::Schedule {
            expr: Some("*/5 * * * *".to_string()),
            param: None,
        },
    );
    has(
        "GET /count",
        HandlerAddress::Http {
            verb: HttpMethod::Get,
            path: Some("/count".to_string()),
        },
    );
    has(
        "POST /items",
        HandlerAddress::Http {
            verb: HttpMethod::Post,
            path: Some("/items".to_string()),
        },
    );
}

const LEDGER: &str = r#"context shop.ledger

consumes bynk { EventEnvelope }

event Placed = { id: String }

service OnPlaced from Events(Placed) {
  on event(p: Placed) -> Effect[()] {
    ()
  }
}
"#;

#[test]
fn an_event_address_records_its_typed_form() {
    let addresses = test_addresses(
        &[
            ("shop/ledger.bynk", LEDGER),
            (
                "tests/ledger.bynk",
                r#"suite shop.ledger {
  case "an event is delivered" {
    let _ <- OnPlaced.event(Placed { id: "o1" })
    expect true
  }
}
"#,
            ),
        ],
        "shop.ledger",
    );
    assert!(
        addresses
            .iter()
            .any(|(a, t)| a == "OnPlaced event" && *t == HandlerAddress::Event),
        "{addresses:?}"
    );
}

const WS_SUITE: &str = r#"suite chat {
  case "a lifecycle" {
    let tc = TestConnection[ServerFrame]()
    let room = RoomId.unsafe("r1")
    let _ <- Gateway.open(tc, room) by Participant(UserId.unsafe("alice"))
    let _ <- Gateway.message(tc, ClientFrame { text: "hi" })
    let _ <- Gateway.close(tc)
    expect tc.closed() == false
  }
}
"#;

const CHAT: &str = r#"context chat

type RoomId = opaque String
type UserId = opaque String
type ServerFrame = { text: String }
type ClientFrame = { text: String }

actor Participant { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

service Gateway from websocket(in: ClientFrame, out: ServerFrame) {
  on open (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).join(user.identity, connection)
    ()
  }
  on message (roomId: RoomId, frame: ClientFrame) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).post(user.identity, frame.text)
    ()
  }
  on close (roomId: RoomId) -> Effect[()] by user: Participant {
    let _ <- Room(roomId).leave(user.identity)
    ()
  }
}

agent Room {
  key id: RoomId
  store members: Set[UserId]
  store conns: Map[UserId, Connection[ServerFrame]]

  on call join(u: UserId, conn: Connection[ServerFrame]) -> Effect[()] {
    let _ <- members.add(u)
    let _ <- conns.put(u, conn)
    ()
  }
  on call leave(u: UserId) -> Effect[()] {
    let _ <- members.remove(u)
    let _ <- conns.remove(u)
    ()
  }
  on call post(sender: UserId, text: String) -> Effect[()] {
    let _ <- conns.parTraverse((c: Connection[ServerFrame]) => c.send(ServerFrame { text: text }))
    ()
  }
}
"#;

#[test]
fn a_websocket_lifecycle_address_records_its_typed_form() {
    let addresses = test_addresses(
        &[("chat.bynk", CHAT), ("tests/chat.bynk", WS_SUITE)],
        "chat",
    );
    let has = |text: &str, typed: WsAddress| {
        assert!(
            addresses
                .iter()
                .any(|(a, t)| a == text && *t == HandlerAddress::Ws(typed.clone())),
            "{text}: {typed:?} missing from {addresses:?}"
        );
    };
    let a = analyse(&[("chat.bynk", CHAT), ("tests/chat.bynk", WS_SUITE)]);
    assert!(
        a.callees_in("chat").iter().any(|c| matches!(
            c,
            Callee::Intrinsic {
                op: StaticOp::TestConnection(TestConnectionStatic::New),
                ..
            }
        )),
        "`TestConnection[F]()` is the `new` intrinsic"
    );
    has("Gateway ws open", WsAddress::Open);
    // `roomId` is the first of two parameters, so the frame is the second.
    has(
        "Gateway ws message",
        WsAddress::Message {
            frame: Some(WsMessageShape {
                frame_index: 1,
                params: 2,
            }),
        },
    );
    has("Gateway ws close", WsAddress::Close { params: Some(1) });
}

/// `run.id`, a test run's identity, is its own intrinsic.
#[test]
fn run_id_records_the_test_run_intrinsic() {
    let a = analyse(&[
        (
            "shop/ledger.bynk",
            "context shop.ledger\n\nservice s {\n  on call() -> Effect[Int] by Caller { 1 }\n}\n",
        ),
        (
            "tests/ledger.bynk",
            "suite shop.ledger {\n  case \"a run has an id\" {\n    expect run.id != \"\"\n  }\n}\n",
        ),
    ]);
    a.assert_clean();
    assert!(
        a.callees_in("shop.ledger").iter().any(|c| matches!(
            c,
            Callee::Intrinsic {
                ns: IntrinsicNs::TestRun,
                op: StaticOp::TestRun(TestRunStatic::Id),
                ..
            }
        )),
        "{:?}",
        a.callees_in("shop.ledger")
    );
}

// ---- call-site principals ---------------------------------------------------

/// Each principal clause is recorded under its own id: an ordinary actor as an actor, `Nobody`,
/// and every bad credential. (The tier and route rules report errors for some of these; the
/// classification is recorded regardless.)
#[test]
fn every_call_site_principal_is_recorded_once_and_an_actor_is_not_reserved() {
    use crate::actors::{BadCredential, CallSitePrincipal};
    let mut cases = String::new();
    for who in [
        "Visitor",
        "Nobody",
        "Forged",
        "Stale",
        "Expired",
        "WrongIssuer",
        "WrongAudience",
        "WrongAlgorithm",
        "NoSubject",
    ] {
        cases.push_str(&format!(
            "  case \"{who}\" {{\n    let r <- api.GET(\"/count\") by {who}\n    expect true\n  }}\n"
        ));
    }
    let a = analyse(&[
        ("cr/jobs.bynk", JOBS),
        (
            "tests/jobs.bynk",
            &format!("suite cr.jobs as integration {{\n{cases}}}\n"),
        ),
    ]);
    let recorded: Vec<CallSitePrincipal> = a
        .callees_in("cr.jobs")
        .iter()
        .filter_map(|c| match c {
            Callee::Principal(p) => Some(p.clone()),
            _ => None,
        })
        .collect();
    assert!(
        recorded.contains(&CallSitePrincipal::Actor("Visitor".to_string())),
        "{recorded:?}"
    );
    assert!(
        recorded.contains(&CallSitePrincipal::Nobody),
        "{recorded:?}"
    );
    for bad in BadCredential::ALL {
        assert!(
            recorded.contains(&CallSitePrincipal::BadCredential(bad)),
            "{bad:?} missing from {recorded:?}"
        );
    }
    assert_eq!(recorded.iter().filter(|p| !p.is_reserved()).count(), 1);
}

/// #934's class: a same-named, non-first-party `Events` whose operation is `emit` does not
/// count as an emit (it used to, by name), while bynk's own `Events.emit` does.
#[test]
fn only_bynks_own_events_emit_counts_as_an_emit() {
    use crate::project_model::is_events_emit;
    let other = analyse(&[
        ("vendor.bynk", OTHER_ADAPTER),
        ("vendor.binding.ts", OTHER_BINDING),
        (
            "demo/orders.bynk",
            "context demo.orders\n\nconsumes vendor { Events }\n\nservice reserve {\n  on call() -> Effect[()] by Caller given Events {\n    Events.emit(\"order\")\n  }\n}\n",
        ),
    ]);
    other.assert_clean();
    let callees = other.callees_in("demo.orders");
    assert!(callees.iter().any(
        |c| matches!(c, Callee::Capability { cap, op, .. } if cap == "Events" && op == "emit")
    ));
    assert!(!callees.iter().any(is_events_emit), "{callees:?}");

    let own = analyse(&[(
        "shop/hooks.bynk",
        "context shop.hooks\n\nconsumes bynk { Events }\n\nevent Seen = { id: String }\n\nservice api {\n  on call(id: String) -> Effect[()] by Caller given Events {\n    Events.emit[Seen](Seen { id: id })\n  }\n}\n",
    )]);
    own.assert_clean();
    assert!(own.callees_in("shop.hooks").iter().any(is_events_emit));
}

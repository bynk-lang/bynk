//! #611: the enumerable storage-operation registry.
//!
//! The entry operations of the `store` kinds (`Cell`/`Map`/`Set`/`Cache`/
//! `Log`) are dispatched by the checker in [`crate::checker`]'s
//! `check_store_*_op` functions, where the operation names live in `match`
//! arms — authoritative for *typing*, but not enumerable. This module is the
//! enumerable view the LSP reads for hover on a store operation, mirroring
//! [`crate::kernel_methods`]'s relationship to the value-kernel dispatch.
//!
//! The signatures are human-readable Bynk-surface display strings, generic in
//! the kind's element/key/value type (`K`/`V`/`T` — the store field's declared
//! kind grounds them at the hover site).
//!
//! **What the drift test pins, and what it does not.**
//! `store_op_registry_pins_dispatch` drives every listed operation through the
//! real checker on a `store` field of the matching kind and asserts none is
//! rejected as `unknown_op` — so the table cannot list a **phantom** operation.
//! It does not bite the other way: an operation added to a `check_store_*_op`
//! arm later fails nothing here, and this table will silently **under-list** it
//! (hover then falls through — a missing hover, not a wrong one). Nor does it
//! check the signature **strings**, which are display-only and unread by the
//! checker; those are pinned by eye against the `check_store_*_op` arms.
//! [`crate::kernel_methods`] has the same shape and the same two limits.

/// One storage operation: its name, a display signature, and whether it
/// mutates the field (a write the handler's implicit commit persists).
#[derive(Debug, Clone, Copy)]
pub struct StoreOp {
    pub name: &'static str,
    pub signature: &'static str,
    /// #2194: `true` for a write (`put`, `append`, `update`, …), `false` for a
    /// read or a lazy query root. The one source for "does this op write
    /// state" — read by `bynk_lower::body_writes_state` (commit detection) and
    /// by the IDE's sequence diagram (state-write notes).
    pub mutates: bool,
}

/// A read-only operation.
const fn op(name: &'static str, signature: &'static str) -> StoreOp {
    StoreOp {
        name,
        signature,
        mutates: false,
    }
}

/// A mutating operation.
const fn write(name: &'static str, signature: &'static str) -> StoreOp {
    StoreOp {
        name,
        signature,
        mutates: true,
    }
}

/// `store Map[K, V]` (v0.82, ADR 0110) — entry-level and effectful.
pub const MAP_STORE_OPS: &[StoreOp] = &[
    write("put", "put(key: K, value: V) -> Effect[()]"),
    op("get", "get(key: K) -> Effect[Option[V]]"),
    write("remove", "remove(key: K) -> Effect[()]"),
    op("contains", "contains(key: K) -> Effect[Bool]"),
    op("size", "size() -> Effect[Int]"),
    write("update", "update(key: K, f: (V) -> V) -> Effect[()]"),
    write(
        "upsert",
        "upsert(key: K, initial: V, f: (V) -> V) -> Effect[()]",
    ),
];

/// The `.entries`/`.keys`/`.values` lazy query accessors a `store Map[K, V]`
/// field exposes (v0.158, ADR 0184) — **fields**, not method calls, so they're
/// dispatched in `crate::checker::expressions::check_field_access` rather than
/// a `check_store_*_op` function, and are a separate table from
/// [`MAP_STORE_OPS`] though they share a receiver. Never offered on a held
/// `Map[K, Connection]` (`bynk.held.query_accessor_on_held_map`) — this static
/// table doesn't carry the value type, so callers gate that themselves.
pub const MAP_QUERY_ACCESSORS: &[StoreOp] = &[
    op("entries", "entries: Query[MapEntry[K, V]]"),
    op("keys", "keys: Query[K]"),
    op("values", "values: Query[V]"),
];

/// `store Cache[K, V]` (v0.87, ADR 0113) — the storage `Map`'s operation set,
/// but its own table rather than an alias: every operation **but `remove`**
/// applies TTL expiry, which reads the clock, so the handler must declare
/// `given Clock`. That requirement is part of the operation's contract, so it is
/// rendered in the signature (as a handler's own `given` clause is written,
/// after the return type) — an alias to [`MAP_STORE_OPS`] would silently drop it.
pub const CACHE_STORE_OPS: &[StoreOp] = &[
    write("put", "put(key: K, value: V) -> Effect[()] given Clock"),
    op("get", "get(key: K) -> Effect[Option[V]] given Clock"),
    // The one op that does not apply expiry, and so does not read the clock.
    write("remove", "remove(key: K) -> Effect[()]"),
    op("contains", "contains(key: K) -> Effect[Bool] given Clock"),
    op("size", "size() -> Effect[Int] given Clock"),
    write(
        "update",
        "update(key: K, f: (V) -> V) -> Effect[()] given Clock",
    ),
    write(
        "upsert",
        "upsert(key: K, initial: V, f: (V) -> V) -> Effect[()] given Clock",
    ),
];

/// `store Set[T]` (v0.83) — entry-level and effectful. Set algebra
/// (`union`/`intersection`/`difference`) is deferred.
pub const SET_STORE_OPS: &[StoreOp] = &[
    write("add", "add(item: T) -> Effect[()]"),
    write("remove", "remove(item: T) -> Effect[()]"),
    op("contains", "contains(item: T) -> Effect[Bool]"),
    op("size", "size() -> Effect[Int]"),
];

/// `store Cell[T]` (v0.98, ADR 0125) — `update` is the only method-shaped
/// operation; a cell is read by its bare name and written with `:=`.
pub const CELL_STORE_OPS: &[StoreOp] = &[write("update", "update(f: (T) -> T) -> Effect[()]")];

/// `store Log[T]` (v0.95, ADR 0121) — `append` is the one effectful write; the
/// time-window roots are lazy `Query[T]` builders. The general query
/// vocabulary the roots feed into is the kernel `Query` surface, not a `Log`
/// operation, so it is not listed here.
pub const LOG_STORE_OPS: &[StoreOp] = &[
    write("append", "append(entry: T) -> Effect[()]"),
    op("since", "since(start: Instant) -> Query[T]"),
    op("before", "before(end: Instant) -> Query[T]"),
    op(
        "between",
        "between(start: Instant, end: Instant) -> Query[T]",
    ),
    op("recent", "recent(count: Int) -> Query[T]"),
    op("reversed", "reversed() -> Query[T]"),
];

/// The operations of the storage kind named `head` (a `StoreKind`'s head
/// identifier — `"Map"`, `"Cell"`, …). `Queue` is in the storage-kind
/// catalogue but has no dispatched operations yet, so it — like an unknown
/// head — yields none.
pub fn ops_for(head: &str) -> &'static [StoreOp] {
    match head {
        "Map" => MAP_STORE_OPS,
        "Cache" => CACHE_STORE_OPS,
        "Set" => SET_STORE_OPS,
        "Cell" => CELL_STORE_OPS,
        "Log" => LOG_STORE_OPS,
        _ => &[],
    }
}

/// The storage kinds with dispatched operations — every head [`ops_for`]
/// maps to a non-empty table.
const KINDS: &[&str] = &["Map", "Cache", "Set", "Cell", "Log"];

/// #2194: does `op` write a `store` field of kind `head`? `false` for a read,
/// a query root, an unknown op or an unknown head.
pub fn is_mutating(head: &str, op: &str) -> bool {
    ops_for(head).iter().any(|o| o.name == op && o.mutates)
}

/// Does `op` write a `store` field of *some* kind? For a caller that already
/// holds the op resolved against its own field's kind (a `Callee::Store`), so
/// the op name alone is unambiguous.
pub fn is_mutating_any(op: &str) -> bool {
    KINDS.iter().any(|head| is_mutating(head, op))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mutating subset per kind — pinned so a flag flipped by mistake in a
    /// table shows up here, not as a missed commit (`body_writes_state`) or a
    /// missing state-write note.
    #[test]
    fn mutating_ops_are_exactly_the_writes() {
        let writes = |head: &str| {
            ops_for(head)
                .iter()
                .filter(|o| o.mutates)
                .map(|o| o.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(writes("Map"), ["put", "remove", "update", "upsert"]);
        assert_eq!(writes("Cache"), ["put", "remove", "update", "upsert"]);
        assert_eq!(writes("Set"), ["add", "remove"]);
        assert_eq!(writes("Cell"), ["update"]);
        assert_eq!(writes("Log"), ["append"]);
        assert!(MAP_QUERY_ACCESSORS.iter().all(|o| !o.mutates));
        assert!(is_mutating("Set", "add"));
        assert!(!is_mutating("Map", "add"));
        assert!(!is_mutating("Map", "get"));
        assert!(!is_mutating("Nope", "put"));
        assert!(is_mutating_any("append"));
        assert!(!is_mutating_any("since"));
    }

    #[test]
    fn ops_for_maps_the_catalogue_and_ignores_unknown_heads() {
        // Assert a *reachable op* per kind, not a table property — `ops_for`
        // mapping a head to the wrong table is exactly what this must catch.
        assert!(ops_for("Map").iter().any(|o| o.name == "put"));
        assert!(ops_for("Cache").iter().any(|o| o.name == "put"));
        assert!(ops_for("Set").iter().any(|o| o.name == "add"));
        assert!(ops_for("Cell").iter().all(|o| o.name == "update"));
        assert!(ops_for("Log").iter().any(|o| o.name == "append"));
        // A `Set` has no `put` and a `Map` no `add` — the tables are distinct.
        assert!(!ops_for("Set").iter().any(|o| o.name == "put"));
        assert!(!ops_for("Map").iter().any(|o| o.name == "add"));
        // `Queue` is a catalogue kind with no dispatched ops; `Nope` is unknown.
        assert!(ops_for("Queue").is_empty());
        assert!(ops_for("Nope").is_empty());
    }

    /// A `Cache` op reads the clock for TTL expiry — every one but `remove` — so
    /// its signature says so. This is the table's reason for existing separately
    /// from [`MAP_STORE_OPS`]; an alias would drop the requirement silently.
    #[test]
    fn cache_signatures_carry_the_clock_requirement_except_remove() {
        for o in CACHE_STORE_OPS {
            let wants_clock = o.name != "remove";
            assert_eq!(
                o.signature.contains("given Clock"),
                wants_clock,
                "`Cache.{}` clock requirement is misrendered: {:?}",
                o.name,
                o.signature
            );
        }
        // The op *names* still mirror the storage `Map`'s set exactly.
        let names = |ops: &[StoreOp]| ops.iter().map(|o| o.name).collect::<Vec<_>>();
        assert_eq!(names(CACHE_STORE_OPS), names(MAP_STORE_OPS));
        // …and the storage `Map` itself requires no clock.
        assert!(!MAP_STORE_OPS.iter().any(|o| o.signature.contains("given")));
    }

    #[test]
    fn signatures_lead_with_their_operation_name() {
        for ops in [
            MAP_STORE_OPS,
            CACHE_STORE_OPS,
            SET_STORE_OPS,
            CELL_STORE_OPS,
            LOG_STORE_OPS,
            MAP_QUERY_ACCESSORS,
        ] {
            assert!(!ops.is_empty());
            for o in ops {
                assert!(!o.name.is_empty());
                assert!(
                    o.signature.starts_with(o.name),
                    "signature {:?} should lead with {:?}",
                    o.signature,
                    o.name
                );
            }
        }
    }

    /// #596: names the exact ADR 0184 accessor set — `entries`/`keys`/`values`
    /// — so a rename/reorder in `check_field_access`'s `map_query` dispatch
    /// surfaces here too.
    #[test]
    fn map_query_accessors_are_entries_keys_values() {
        let names: Vec<&str> = MAP_QUERY_ACCESSORS.iter().map(|o| o.name).collect();
        assert_eq!(names, vec!["entries", "keys", "values"]);
    }
}

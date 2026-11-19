//! The method-call dispatchers, one file per family (back-half paydown 1A.20, #3721,
//! Decision E). `lower.rs` keeps `lower_method_call`; each file here holds the
//! dispatchers of one family, moved byte for byte before they convert to nodes.

pub(super) mod codec_statics;
pub(super) mod cross_context;
pub(super) mod platform;
pub(super) mod query;
pub(super) mod statics;
pub(super) mod store;

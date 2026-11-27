//! The method-call dispatchers, one file per family (back-half paydown 1A.20, #3721,
//! Decision E). `lower/calls.rs` keeps `lower_method_call`; each file here holds the
//! dispatchers of one family, moved byte for byte before they convert to nodes.

pub(super) mod agents;
pub(crate) mod api_key;
pub(crate) mod codec_statics;
pub(super) mod cross_context;
pub(super) mod kernel;
pub(super) mod kernel_collections;
pub(super) mod kernel_container;
pub(super) mod kernel_scalar;
pub(super) mod platform;
pub(super) mod query;
pub(super) mod statics;
pub(super) mod store;
pub(super) mod test_calls;

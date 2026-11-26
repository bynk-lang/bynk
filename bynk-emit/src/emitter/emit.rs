//! Per-declaration emission — the functions `emit_project` drives to render each top-level Bynk
//! declaration into TypeScript. Split out of `emitter.rs` (ADR 0060), then into one file per
//! declaration kind (back-half paydown 2.1, #4086). This file only declares those files and
//! re-exports them, so every `emitter::emit::X` and `emitter::X` path still resolves; each child
//! imports what it uses explicitly. The one glob import below is not for this file: moved function
//! bodies spell parent paths through it, such as `super::tco::plan`, `super::commons_alias`,
//! `super::serialisation::…` and `super::ty_to_type_ref`.
//!
//! - `types`: type, refined, record and sum declarations and their checks;
//! - `functions`: attached methods, free functions, constants and contract guards;
//! - `suite`: the declarations a test suite makes, which `testgen` calls;
//! - `handlers`: handler method names and labels, and the handler-body items services, agents and
//!   free functions share;
//! - `messages`: `messages` bundles and ICU placeholders;
//! - `capability`: capabilities, derived capabilities and providers;
//! - `service`: services, contexts, the `make` surface and the actor-sum types;
//! - `websocket`: the Durable Object methods and `fetch` upgrade branch of a `from websocket`
//!   service;
//! - `agent`: `emit_agent`, the agent class's constructor, its state record and its zero entries;
//! - `agent_store`: an agent's store layout, lazy-read specs, and key and wire codecs;
//! - `agent_state`: the `loadState` and `commitState` functions, hydration and the persistence
//!   guards;
//! - `agent_migrate`: the `migrate from N` step functions and their dispatcher;
//! - `agent_dispatch`: an agent's handler methods, Workers `fetch` dispatch and factory;
//! - `agent_history`: the history-property driver.

use super::*;

mod agent;
mod agent_dispatch;
mod agent_history;
mod agent_migrate;
mod agent_state;
mod agent_store;
mod capability;
mod functions;
mod handlers;
mod messages;
mod service;
mod suite;
mod types;
mod websocket;
mod websocket_close;

pub(crate) use agent::*;
use agent_dispatch::*;
use agent_history::*;
use agent_migrate::*;
use agent_state::*;
pub(crate) use agent_store::*;
pub(crate) use capability::*;
pub(crate) use functions::*;
pub use handlers::*;
pub(crate) use messages::*;
pub use service::*;
pub use suite::*;
pub(crate) use types::*;
pub(crate) use websocket::*;

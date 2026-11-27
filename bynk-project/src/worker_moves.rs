//! #3565: the phase variants a build writes for a supported agent move
//! (`design/tracks/agent-context-moves.md` §3.2). `bynk deploy` decides them from its ledger
//! and hands them to the build through the [`WorkerLayout`](crate::worker_layout::WorkerLayout);
//! the emitter (`bynk_emit::emitter::workers_moves`) writes them. Empty for every other build.

use std::collections::BTreeMap;

/// One context a Worker receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    /// The moving context's qualified name.
    pub context: String,
    /// Its agent classes.
    pub classes: Vec<String>,
    /// The script it is moving from.
    pub from_script: String,
}

/// One class a Worker gives up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub class: String,
    /// The script it is moving to.
    pub to_script: String,
}

/// The phase variants a build writes, by Worker directory.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MoveVariants {
    /// Each receiving Worker of the build → what it receives.
    pub receive: BTreeMap<String, Vec<Incoming>>,
    /// Each source Worker of the build → the classes it gives up.
    pub commit: BTreeMap<String, Vec<Outgoing>>,
    /// Each source the build no longer has (its directory → its script name and the classes
    /// it gives up), pushed as a stub in the commit phase.
    pub stubs: BTreeMap<String, (String, Vec<Outgoing>)>,
}

impl MoveVariants {
    /// Whether the build writes no variant.
    pub fn is_empty(&self) -> bool {
        self.receive.is_empty() && self.commit.is_empty() && self.stubs.is_empty()
    }
}

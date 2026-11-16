//! Where each context's generated files go under a Workers build (#3562, G2b).
//!
//! A context is one Worker, and its files go under `workers/<dir>/`, `<dir>`
//! being the dashed context name. A `[deploy.workers.<key>]` group puts several
//! contexts in one Worker, so they share one directory, `workers/<key>/`, and
//! each member's own files carry a prefix (`<member>.handlers.ts`) so two
//! members never write the same file. The files that belong to the Worker as a
//! whole (`index.ts`, `wrangler.toml`) take no prefix.
//!
//! [`WorkerLayout::identity`] is the layout of a project with no group: every
//! context in a directory of its own, every file under its usual name.

use crate::paths::worker_dir_name;
use crate::worker_groups::WorkerGroups;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A context's place in a Worker: the Worker's directory, and the prefix its
/// own files take when other contexts share the directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    dir: String,
    /// `Some` when the context is a member of a group: its file prefix.
    prefix: Option<String>,
}

/// Where each context's generated files go.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerLayout {
    /// The contexts that are members of a group; every other context is
    /// [`Self::place`]d by its dashed name.
    members: BTreeMap<String, Place>,
}

impl WorkerLayout {
    /// The layout of a project with no `[deploy.workers]` group.
    pub fn identity() -> Self {
        Self::default()
    }

    /// The layout a project's build uses, from its `bynk.toml` (with `overlay`
    /// over the files on disk). The one place the build, the checker and the
    /// editor learn where each context's Worker is, so they cannot disagree.
    /// Identity for now: groups are switched on with the group as the emit unit.
    pub fn for_project(
        _project_root: &std::path::Path,
        _overlay: &std::collections::HashMap<PathBuf, String>,
    ) -> Self {
        Self::identity()
    }

    /// The layout of `groups`: a member goes in its group's directory, under a
    /// prefix; every other context is as in [`Self::identity`].
    pub fn from_groups(groups: &WorkerGroups) -> Self {
        let members = groups
            .iter()
            .flat_map(|(key, group)| {
                group.contexts.iter().map(move |ctx| {
                    (
                        ctx.clone(),
                        Place {
                            dir: key.clone(),
                            prefix: Some(worker_dir_name(ctx)),
                        },
                    )
                })
            })
            .collect();
        Self { members }
    }

    fn place(&self, context: &str) -> Place {
        self.members.get(context).cloned().unwrap_or_else(|| Place {
            dir: worker_dir_name(context),
            prefix: None,
        })
    }

    /// The directory name, under `workers/`, of the Worker `context` is in.
    pub fn dir(&self, context: &str) -> String {
        self.place(context).dir
    }

    /// The path of a file that belongs to the Worker as a whole (`index.ts`,
    /// `wrangler.toml`): `workers/<dir>/<name>`.
    pub fn worker_file(&self, context: &str, name: &str) -> PathBuf {
        PathBuf::from(format!("workers/{}/{name}", self.dir(context)))
    }

    /// The path of one of `context`'s own files: `workers/<dir>/<name>`, or,
    /// when other contexts share the directory, `workers/<dir>/<member>.<name>`.
    pub fn member_file(&self, context: &str, name: &str) -> PathBuf {
        let place = self.place(context);
        match place.prefix {
            Some(prefix) => PathBuf::from(format!("workers/{}/{prefix}.{name}", place.dir)),
            None => PathBuf::from(format!("workers/{}/{name}", place.dir)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_groups::WorkerGroup;

    fn groups() -> WorkerGroups {
        WorkerGroups::from([(
            "core".to_string(),
            WorkerGroup {
                contexts: vec!["shop.orders".to_string(), "shop.pricing".to_string()],
                ..WorkerGroup::default()
            },
        )])
    }

    /// With no group, a context's files are where they have always been.
    #[test]
    fn identity_is_one_directory_per_context() {
        let layout = WorkerLayout::identity();
        assert_eq!(layout.dir("shop.orders"), "shop-orders");
        assert_eq!(
            layout.worker_file("shop.orders", "wrangler.toml"),
            PathBuf::from("workers/shop-orders/wrangler.toml")
        );
        assert_eq!(
            layout.member_file("shop.orders", "compose.ts"),
            PathBuf::from("workers/shop-orders/compose.ts")
        );
    }

    /// A group's members share its directory, each under its own prefix; a
    /// Worker-wide file takes none.
    #[test]
    fn members_share_the_groups_directory_under_a_prefix() {
        let layout = WorkerLayout::from_groups(&groups());
        assert_eq!(layout.dir("shop.pricing"), "core");
        assert_eq!(
            layout.member_file("shop.orders", "compose.ts"),
            PathBuf::from("workers/core/shop-orders.compose.ts")
        );
        assert_eq!(
            layout.member_file("shop.pricing", "compose.ts"),
            PathBuf::from("workers/core/shop-pricing.compose.ts")
        );
        assert_eq!(
            layout.worker_file("shop.pricing", "index.ts"),
            PathBuf::from("workers/core/index.ts")
        );
    }

    /// A context in no group stays as in the identity layout.
    #[test]
    fn an_ungrouped_context_is_unchanged() {
        let layout = WorkerLayout::from_groups(&groups());
        assert_eq!(layout.dir("shop.payment"), "shop-payment");
        assert_eq!(
            layout.member_file("shop.payment", "compose.ts"),
            PathBuf::from("workers/shop-payment/compose.ts")
        );
    }
}

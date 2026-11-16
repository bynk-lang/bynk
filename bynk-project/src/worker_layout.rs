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

    /// The project-relative synthetic source path of `context`'s handlers
    /// module, which the emitter's relative-import machinery resolves against.
    pub fn handlers_source_path(&self, context: &str) -> PathBuf {
        self.member_file(context, "handlers.bynk")
    }

    /// The project-relative output path of `context`'s handlers module.
    pub fn handlers_output_path(&self, context: &str) -> PathBuf {
        self.member_file(context, "handlers.ts")
    }

    /// The synthetic source path of one file's module in a context split across
    /// files (#1820): `handlers/<stem>.bynk` beside the handlers barrel.
    pub fn file_source_path(&self, context: &str, source: &std::path::Path) -> PathBuf {
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        self.member_file(context, &format!("handlers/{stem}.bynk"))
    }

    /// The specifier by which one of `context`'s own modules (its compose and
    /// entry) imports its handlers module: `./handlers.js`, or
    /// `./<member>.handlers.js` in a group.
    pub fn own_handlers_specifier(&self, context: &str) -> String {
        format!("./{}", self.handlers_file_stem(context))
    }

    fn handlers_file_stem(&self, context: &str) -> String {
        let file = self.handlers_output_path(context);
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        format!("{}.js", name.trim_end_matches(".ts"))
    }

    /// The import specifier, from a module in `from`'s Worker directory, of
    /// `to`'s handlers module: `../<dir>/handlers.js` across Workers,
    /// `./<member>.handlers.js` within one.
    pub fn handlers_specifier(&self, from: &str, to: &str) -> String {
        let name = self.handlers_file_stem(to);
        // A context naming its own handlers from a module one directory down
        // (a split context's) goes up and back in, as it always has.
        if from != to && self.dir(from) == self.dir(to) {
            format!("./{name}")
        } else {
            format!("../{}/{name}", self.dir(to))
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

    /// Without a group the handlers paths and specifiers are the ones every
    /// Workers build has always used.
    #[test]
    fn identity_handlers_paths_are_unchanged() {
        let layout = WorkerLayout::identity();
        assert_eq!(
            layout.handlers_source_path("shop.orders"),
            PathBuf::from("workers/shop-orders/handlers.bynk")
        );
        assert_eq!(
            layout.handlers_output_path("shop.orders"),
            PathBuf::from("workers/shop-orders/handlers.ts")
        );
        assert_eq!(
            layout.file_source_path(
                "shop.orders",
                std::path::Path::new("shop/orders/place.bynk")
            ),
            PathBuf::from("workers/shop-orders/handlers/place.bynk")
        );
        assert_eq!(
            layout.own_handlers_specifier("shop.orders"),
            "./handlers.js"
        );
        assert_eq!(
            layout.handlers_specifier("shop.orders", "shop.pricing"),
            "../shop-pricing/handlers.js"
        );
    }

    /// In a group a member's handlers sit beside its siblings' under its prefix,
    /// and an import within the Worker is local; one out of it is not.
    #[test]
    fn members_handlers_are_prefixed_and_imported_locally() {
        let layout = WorkerLayout::from_groups(&groups());
        assert_eq!(
            layout.handlers_output_path("shop.pricing"),
            PathBuf::from("workers/core/shop-pricing.handlers.ts")
        );
        assert_eq!(
            layout.file_source_path("shop.orders", std::path::Path::new("o/place.bynk")),
            PathBuf::from("workers/core/shop-orders.handlers/place.bynk")
        );
        assert_eq!(
            layout.handlers_specifier("shop.orders", "shop.pricing"),
            "./shop-pricing.handlers.js"
        );
        assert_eq!(
            layout.handlers_specifier("shop.orders", "billing"),
            "../billing/handlers.js"
        );
    }

    /// A context importing its own handlers (a split context's agent deps) keeps
    /// the specifier it has always had, so a build without groups is unchanged.
    #[test]
    fn a_contexts_own_handlers_import_keeps_its_spelling() {
        assert_eq!(
            WorkerLayout::identity().handlers_specifier("shop.orders", "shop.orders"),
            "../shop-orders/handlers.js"
        );
    }
}

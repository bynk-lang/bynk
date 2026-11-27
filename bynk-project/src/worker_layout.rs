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
    /// The Worker's name on Cloudflare: the group's `name`, else its key; a
    /// context's own Worker is named by its dashed name.
    script: String,
}

/// Where each context's generated files go.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerLayout {
    /// The contexts that are members of a group; every other context is
    /// [`Self::place`]d by its dashed name.
    members: BTreeMap<String, Place>,
    /// Each group's members in the order its `contexts` list gives them, by the
    /// group's directory: the order the group's entry tries them in.
    order: BTreeMap<String, Vec<String>>,
    /// #4127: each group's own Workers Logs settings, by the group's directory.
    observability: BTreeMap<String, crate::observability::ObservabilityOverride>,
    /// #4128: each group's own CPU limit and placement, by the group's directory.
    tuning: BTreeMap<String, crate::worker_tuning::WorkerTuningOverride>,
    /// #3883: the `[deploy.email]` mapping, so a Worker's `email()` accepts the address each
    /// environment claims in place of a `from email` service's own.
    email: crate::email_deploy::EmailDeployConfig,
    /// #3565: the agent-move phase variants the build writes; empty but mid-move.
    moves: crate::worker_moves::MoveVariants,
}

impl WorkerLayout {
    /// #3565: this layout, with the agent-move phase variants the build is to write.
    pub fn with_moves(mut self, moves: crate::worker_moves::MoveVariants) -> Self {
        self.moves = moves;
        self
    }

    /// #3565: the agent-move phase variants the build writes.
    pub fn moves(&self) -> &crate::worker_moves::MoveVariants {
        &self.moves
    }

    /// The layout of a project with no `[deploy.workers]` group.
    pub fn identity() -> Self {
        Self::default()
    }

    /// The layout a project's build uses, from its `bynk.toml` (with `overlay`
    /// over the files on disk). The one place the build, the checker and the
    /// editor learn where each context's Worker is, so they cannot disagree.
    /// A manifest with no `[deploy.workers]` table, or one that does not read
    /// (the manifest checks report that), is [`Self::identity`].
    pub fn for_project(
        project_root: &std::path::Path,
        overlay: &std::collections::HashMap<PathBuf, String>,
    ) -> Self {
        let mut layout =
            match crate::worker_groups::try_read_worker_groups_with(project_root, overlay) {
                Ok(groups) if !groups.is_empty() => Self::from_groups(&groups),
                _ => Self::identity(),
            };
        layout.email = crate::email_deploy::try_read_email_deploy_with(project_root, overlay)
            .unwrap_or_default();
        layout
    }

    /// #3883: the project's `[deploy.email]` mapping, as the build read it.
    pub fn email_deploy(&self) -> &crate::email_deploy::EmailDeployConfig {
        &self.email
    }

    /// #3883: the addresses a named environment claims in place of the `from email` address
    /// `source`, sorted. A Worker accepts mail to each beside the source's own.
    pub fn email_aliases(&self, source: &str) -> Vec<String> {
        crate::email_deploy::mapped_addresses(&self.email, source)
    }

    /// The layout of `groups`: a member goes in its group's directory, under a
    /// prefix; every other context is as in [`Self::identity`].
    pub fn from_groups(groups: &WorkerGroups) -> Self {
        let order = groups
            .iter()
            .map(|(key, group)| (key.clone(), group.contexts.clone()))
            .collect();
        let members = groups
            .iter()
            .flat_map(|(key, group)| {
                group.contexts.iter().map(move |ctx| {
                    (
                        ctx.clone(),
                        Place {
                            dir: key.clone(),
                            prefix: Some(worker_dir_name(ctx)),
                            script: group.script_name(key).to_string(),
                        },
                    )
                })
            })
            .collect();
        let observability = groups
            .iter()
            .filter_map(|(key, group)| group.observability.map(|o| (key.clone(), o)))
            .collect();
        let tuning = groups
            .iter()
            .filter_map(|(key, group)| group.tuning.map(|t| (key.clone(), t)))
            .collect();
        Self {
            members,
            order,
            observability,
            tuning,
            email: Default::default(),
            moves: Default::default(),
        }
    }

    /// #4128: the CPU limit and placement of the Worker `context` belongs to:
    /// `base` (the project's), with its group's keys laid over it.
    pub fn tuning(
        &self,
        context: &str,
        base: crate::worker_tuning::WorkerTuning,
    ) -> crate::worker_tuning::WorkerTuning {
        self.tuning
            .get(&self.dir(context))
            .map_or(base, |t| t.over(base))
    }

    /// #4127: the Workers Logs settings of the Worker `context` belongs to:
    /// `base` (the project's), with its group's table laid over it.
    pub fn observability(
        &self,
        context: &str,
        base: crate::observability::Observability,
    ) -> crate::observability::Observability {
        self.observability
            .get(&self.dir(context))
            .map_or(base, |o| o.over(base))
    }

    /// Whether `context` is a member of a group, so its entry is one of several
    /// behind the group's dispatcher and marks the requests it does not own.
    pub fn is_member(&self, context: &str) -> bool {
        self.members.contains_key(context)
    }

    fn place(&self, context: &str) -> Place {
        self.members.get(context).cloned().unwrap_or_else(|| Place {
            dir: worker_dir_name(context),
            prefix: None,
            script: worker_dir_name(context),
        })
    }

    /// The directory name, under `workers/`, of the Worker `context` is in.
    pub fn dir(&self, context: &str) -> String {
        self.place(context).dir
    }

    /// The name of the Worker `context` is in on Cloudflare: what a Service
    /// Binding to it, and its own `wrangler.toml`, call it. The group's `name`
    /// or key for a member, the dashed context name otherwise. (The directory
    /// and the names derived for its resources stay the key, the group's
    /// identity, so that renaming a script does not move them.)
    pub fn script_name(&self, context: &str) -> String {
        self.place(context).script
    }

    /// Every group, by its directory (the group's key), with its members in the
    /// order the manifest lists them.
    pub fn groups(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.order
            .iter()
            .map(|(dir, members)| (dir.as_str(), members.as_slice()))
    }

    /// The members of the group whose directory is `dir`, in the order the
    /// manifest lists them; `None` for a context's own Worker.
    pub fn group_members(&self, dir: &str) -> Option<&[String]> {
        self.order.get(dir).map(Vec::as_slice)
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

    /// The import specifier, from a module in `context`'s Worker directory, of
    /// one of its own files: `./<name>` stripped of its extension and given
    /// `.js`, with the member prefix in a group (`./shop-orders.compose.js`).
    pub fn own_module_specifier(&self, context: &str, name: &str) -> String {
        let file = self.member_file(context, name);
        let file = file.file_name().unwrap_or_default().to_string_lossy();
        let stem = file.rsplit_once('.').map_or(&*file, |(stem, _)| stem);
        format!("./{stem}.js")
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

    /// Only a member of a group is a member: a project with no group has none.
    #[test]
    fn only_a_group_member_is_a_member() {
        assert!(!WorkerLayout::identity().is_member("shop.orders"));
        let layout = WorkerLayout::from_groups(&groups());
        assert!(layout.is_member("shop.orders"));
        assert!(!layout.is_member("shop.payment"));
    }

    /// A Worker's name on Cloudflare is the group's `name`, else its key; a
    /// context's own is its dashed name. A member's own modules are imported under
    /// its prefix, and a group lists its members in manifest order.
    #[test]
    fn a_groups_script_name_modules_and_members() {
        let mut named = groups();
        named.get_mut("core").unwrap().name = Some("acme-core".to_string());
        let layout = WorkerLayout::from_groups(&named);
        assert_eq!(layout.script_name("shop.orders"), "acme-core");
        assert_eq!(layout.dir("shop.orders"), "core");
        assert_eq!(layout.script_name("shop.payment"), "shop-payment");
        assert_eq!(
            layout.own_module_specifier("shop.pricing", "compose.ts"),
            "./shop-pricing.compose.js"
        );
        assert_eq!(
            WorkerLayout::identity().own_module_specifier("shop.pricing", "events_fanout.ts"),
            "./events_fanout.js"
        );
        assert_eq!(
            layout.group_members("core"),
            Some(["shop.orders".to_string(), "shop.pricing".to_string()].as_slice())
        );
        assert_eq!(layout.group_members("shop-payment"), None);
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

#[cfg(test)]
mod email_alias_tests {
    use super::*;

    /// #3883: the layout a project's build uses carries its `[deploy.email]` mapping, with or
    /// without a `[deploy.workers]` group.
    #[test]
    fn the_project_layout_carries_the_email_mapping() {
        let toml = "[project]\nname = \"p\"\nversion = \"0.1.0\"\n\n[deploy.email.\"support@shop.example\"]\naddress = { staging = \"support@staging.shop.example\" }\n";
        let root = std::path::Path::new("/project");
        let overlay = std::collections::HashMap::from([(root.join("bynk.toml"), toml.to_string())]);
        let layout = WorkerLayout::for_project(root, &overlay);
        assert_eq!(
            layout.email_aliases("Support@Shop.example"),
            ["support@staging.shop.example"]
        );
        assert!(layout.email_aliases("sales@shop.example").is_empty());
    }
}

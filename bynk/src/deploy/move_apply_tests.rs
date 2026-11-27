//! #3565 (slice 4a): the move engine against a fake account (`design/tracks/agent-context-moves.md`
//! §3.3, §3.4, §7.1, §7.2). The fake pushes change the fake account as Cloudflare does: a push
//! is a new deployment whose version carries the push's deploy facts, a commit moves each class's
//! namespace to its receiver (same id, S5), and a phase 3 pushed before a commit starts the class
//! empty on the receiver (a namespace of a new id, S6). Every ledger write is watched
//! (`ledger::write_watch`), so each test pins the record after each write.

use super::*;
use crate::deploy::ledger::write_watch;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const ENV: &str = "default";

/// How one scripted push goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    /// It lands, as Cloudflare would apply it.
    Land,
    /// It fails before anything lands.
    Fail,
    /// It is cut short before it lands.
    Interrupt,
    /// It lands, but the run is cut short before it hears so.
    LandThenInterrupt,
    /// A commit refused with `transferred_pending_not_found`.
    Lose,
    /// A commit that lands without the namespace moving.
    LandWithoutCommit,
}

#[derive(Default)]
struct World {
    namespaces: Vec<Namespace>,
    /// Script → deployments, newest first.
    deployments: BTreeMap<String, Vec<Deployment>>,
    /// (script, version) → deploy facts.
    facts: BTreeMap<(String, String), String>,
    n: u32,
    /// Each push, as `worker:phase`, in the order it started.
    pushes: Vec<String>,
    /// (worker, phase) → the next pushes' acts; a push with none lands.
    acts: BTreeMap<(String, u8), VecDeque<Act>>,
}

impl World {
    fn deploy(&mut self, script: &str, facts: Option<String>) -> String {
        self.n += 1;
        let (id, version) = (format!("d{}", self.n), format!("v{}", self.n));
        self.deployments
            .entry(script.to_string())
            .or_default()
            .insert(
                0,
                Deployment {
                    id: id.clone(),
                    created_on: String::new(),
                    trigger: Some("upload".into()),
                    versions: vec![(version.clone(), 100)],
                },
            );
        if let Some(facts) = facts {
            self.facts.insert((script.to_string(), version), facts);
        }
        id
    }

    fn namespace(&mut self, id: &str, script: &str, class: &str) {
        self.namespaces.push(Namespace {
            id: id.into(),
            script: script.into(),
            class: class.into(),
        });
    }

    fn on(&self, class: &str) -> Vec<(String, String)> {
        self.namespaces
            .iter()
            .filter(|n| n.class == class)
            .map(|n| (n.id.clone(), n.script.clone()))
            .collect()
    }
}

#[derive(Clone)]
struct Fake {
    world: Arc<Mutex<World>>,
    /// Worker → script.
    scripts: BTreeMap<String, String>,
}

impl Fake {
    fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }

    fn script(&self, worker: &str) -> String {
        crate::deploy::deployed_worker_name(ENV, &self.scripts[worker])
    }

    fn act(&self, worker: &str, phase: u8, acts: &[Act]) {
        self.world()
            .acts
            .entry((worker.to_string(), phase))
            .or_default()
            .extend(acts.iter().copied());
    }

    fn pushes(&self) -> Vec<String> {
        self.world().pushes.clone()
    }

    /// A deployment of `worker` made outside the move (no facts).
    fn foreign_deploy(&self, worker: &str) -> String {
        let script = self.script(worker);
        self.world().deploy(&script, None)
    }
}

impl MoveAccount for Fake {
    fn namespaces(&self) -> Result<Vec<Namespace>, CloudflareError> {
        Ok(self.world().namespaces.clone())
    }

    fn deployments(&self, script: &str) -> Result<Vec<Deployment>, CloudflareError> {
        Ok(self
            .world()
            .deployments
            .get(script)
            .cloned()
            .unwrap_or_default())
    }

    fn version_facts(
        &self,
        script: &str,
        version: &str,
    ) -> Result<Option<String>, CloudflareError> {
        Ok(self
            .world()
            .facts
            .get(&(script.to_string(), version.to_string()))
            .cloned())
    }
}

impl MovePusher for Fake {
    fn push(&self, worker: &str, phase: Phase, transfer: &Transfer) -> PhasePush {
        let script = self.script(worker);
        let mut world = self.world();
        world.pushes.push(format!("{worker}:{}", phase.number()));
        let act = world
            .acts
            .get_mut(&(worker.to_string(), phase.number()))
            .and_then(VecDeque::pop_front)
            .unwrap_or(Act::Land);
        let failure = || DeployFailure {
            message: format!("wrangler deploy failed for `{script}`"),
            code: 1,
        };
        match act {
            Act::Fail => return PhasePush::Failed(failure()),
            Act::Interrupt => return PhasePush::Interrupted,
            Act::Lose => return PhasePush::TransferLost(failure()),
            Act::Land | Act::LandThenInterrupt | Act::LandWithoutCommit => {}
        }
        let facts = Facts {
            v: 1,
            transfer: Some(transfer.clone()),
            ..Facts::default()
        };
        world.deploy(&script, Some(serde_json::to_string(&facts).unwrap()));
        match phase {
            Phase::Commit if act != Act::LandWithoutCommit => {
                for (class, to) in &transfer.transferred {
                    for n in world.namespaces.iter_mut() {
                        if n.script == script && &n.class == class {
                            n.script = to.clone();
                        }
                    }
                }
            }
            _ => {}
        }
        if act == Act::LandThenInterrupt {
            PhasePush::Interrupted
        } else {
            PhasePush::Landed
        }
    }

    fn record(&self, _lock: &mut DeployLock, _worker: &str, _phase: Phase) {}
}

fn entry(context: &str, class: &str, from: &str, to: &str) -> AgentMoveEntry {
    AgentMoveEntry {
        context: context.into(),
        classes: vec![class.into()],
        from_worker: from.into(),
        from_script: from.into(),
        to_worker: to.into(),
        to_script: to.into(),
    }
}

/// `moves` with each class's namespace on its source, and each Worker deployed once.
fn fake(moves: &[AgentMoveEntry]) -> Fake {
    let mut world = World::default();
    let mut scripts = BTreeMap::new();
    for m in moves {
        scripts.insert(m.from_worker.clone(), m.from_script.clone());
        scripts.insert(m.to_worker.clone(), m.to_script.clone());
        for class in &m.classes {
            let id = format!("ns-{class}");
            world.namespace(&id, &m.from_script, class);
        }
    }
    for script in scripts.values().collect::<BTreeSet<_>>() {
        world.deploy(script, None);
    }
    Fake {
        world: Arc::new(Mutex::new(world)),
        scripts,
    }
}

/// One move: `shop.orders`'s `Cart` from the dissolved `shop-orders` into group `core`.
fn one_move() -> Vec<AgentMoveEntry> {
    vec![entry("shop.orders", "Cart", "shop-orders", "core")]
}

/// Fan-in: `shop.orders` and `shop.billing` join `core`.
fn fan_in() -> Vec<AgentMoveEntry> {
    vec![
        entry("shop.orders", "Cart", "shop-orders", "core"),
        entry("shop.billing", "Ledger", "shop-billing", "core"),
    ]
}

/// Fan-out: group `shop` splits `Cart` to `orders` and `Ledger` to `billing`.
fn fan_out() -> Vec<AgentMoveEntry> {
    vec![
        entry("shop.orders", "Cart", "shop", "orders"),
        entry("shop.billing", "Ledger", "shop", "billing"),
    ]
}

/// A ledger on disk, its record after each write, and a way to make the next writes fail.
struct Ledger {
    lock: DeployLock,
    path: PathBuf,
    writes: Arc<Mutex<Vec<String>>>,
    /// The 1-based write (of all so far) that fails, if any.
    fail_at: Arc<Mutex<Option<usize>>>,
}

impl Ledger {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "bynk-move-apply-{}-{unique}.deploy.lock",
            std::process::id()
        ));
        Ledger {
            lock: toml::from_str("version = 1\n").unwrap(),
            path,
            writes: Arc::default(),
            fail_at: Arc::default(),
        }
    }

    /// Run `f` with a writer over this ledger, every write seen.
    fn with<T>(&mut self, f: impl FnOnce(&mut LedgerWriter<'_>) -> T) -> T {
        let (writes, fail_at) = (self.writes.clone(), self.fail_at.clone());
        let watch: write_watch::Watch = Box::new(move |_, lock| {
            let mut writes = writes.lock().unwrap();
            writes.push(describe(lock));
            if *fail_at.lock().unwrap() == Some(writes.len()) {
                return Err("disk full".into());
            }
            Ok(())
        });
        let (lock, path) = (&mut self.lock, &self.path);
        write_watch::watching(watch, || f(&mut LedgerWriter::new(lock, path)))
    }

    /// The writes since the last call.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.writes.lock().unwrap())
    }

    fn record(&self) -> Option<AgentMoveRecord> {
        self.lock.open_moves(ENV).first().cloned()
    }
}

impl Drop for Ledger {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A ledger's move record as one line: its step and `done`, or `closed`.
fn describe(lock: &DeployLock) -> String {
    match lock.open_moves(ENV).first() {
        None => "closed".into(),
        Some(r) if r.done.is_empty() => step_name(r.step).into(),
        Some(r) => format!("{} done={}", step_name(r.step), r.done.join(",")),
    }
}

fn run(fake: &Fake) -> MoveRun<'_> {
    MoveRun {
        environment: ENV,
        account: fake,
        pusher: fake,
    }
}

/// Open `moves` as move `m1`.
fn open(ledger: &mut Ledger, fake: &Fake, moves: Vec<AgentMoveEntry>) -> Result<(), MoveStop> {
    ledger.with(|w| run(fake).open(w, moves, "m1", "2026-11-27T10:15:02Z"))
}

/// The set's two nodes, as the run's order reaches them.
fn nodes(ledger: &mut Ledger, fake: &Fake) -> Result<(), MoveStop> {
    ledger.with(|w| {
        run(fake).receive(w, "m1")?;
        run(fake).commit(w, "m1")
    })
}

fn codes(stop: &MoveStop) -> Vec<String> {
    stop.refusals.iter().map(|e| e.code.clone()).collect()
}

fn subject(e: &Entry, key: &str) -> serde_json::Value {
    e.subject.get(key).cloned().unwrap_or_default()
}

#[test]
fn a_move_records_each_step_as_its_three_phases_land_and_closes_its_record() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    open(&mut ledger, &fake, one_move()).unwrap();
    let opened = ledger.record().unwrap();
    assert_eq!(opened.step, MoveStep::Receiving);
    assert_eq!(
        opened.namespaces,
        BTreeMap::from([("Cart".into(), "ns-Cart".into())])
    );
    assert_eq!(
        opened.deployments,
        BTreeMap::from([
            ("core".into(), "d1".into()),
            ("shop-orders".into(), "d2".into())
        ])
    );
    assert_eq!(opened.bynk, env!("CARGO_PKG_VERSION"));

    nodes(&mut ledger, &fake).unwrap();
    assert_eq!(fake.pushes(), ["core:1", "shop-orders:2", "core:3"]);
    assert_eq!(
        ledger.take(),
        [
            "receiving",
            "receiving done=core",
            "received",
            "committing",
            "committing done=shop-orders",
            "committed",
            "going_live",
            "going_live done=core",
            "closed",
        ]
    );
    assert!(!ledger.lock.has_open_move());
    // The data is the receiver's, under the namespace it had before the move (S5).
    assert_eq!(fake.world().on("Cart"), [("ns-Cart".into(), "core".into())]);
}

#[test]
fn each_phase_version_carries_the_move_its_phase_and_what_it_expects_or_gave_up() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    open(&mut ledger, &fake, one_move()).unwrap();
    nodes(&mut ledger, &fake).unwrap();
    let world = fake.world();
    let transfer = |script: &str, version: &str| {
        Facts::decode(&world.facts[&(script.to_string(), version.to_string())])
            .unwrap()
            .transfer
            .unwrap()
    };
    let phase1 = transfer("core", "v3");
    assert_eq!((phase1.move_id.as_str(), phase1.phase), ("m1", 1));
    assert_eq!(
        phase1.expecting,
        BTreeMap::from([("Cart".into(), "shop-orders".into())])
    );
    assert!(phase1.transferred.is_empty());
    let phase2 = transfer("shop-orders", "v4");
    assert_eq!(phase2.phase, 2);
    assert_eq!(
        phase2.transferred,
        BTreeMap::from([("Cart".into(), "core".into())])
    );
    let phase3 = transfer("core", "v5");
    assert_eq!(phase3.phase, 3);
    assert!(phase3.expecting.is_empty() && phase3.transferred.is_empty());
}

/// A named environment's moves are between its own scripts (`<script>-<environment>`).
#[test]
fn a_named_environment_moves_between_its_own_scripts() {
    struct Staging(Fake);
    impl MovePusher for Staging {
        fn push(&self, worker: &str, phase: Phase, transfer: &Transfer) -> PhasePush {
            let script = format!("{}-staging", self.0.scripts[worker]);
            let mut world = self.0.world();
            world.pushes.push(format!("{worker}:{}", phase.number()));
            let facts = Facts {
                v: 1,
                transfer: Some(transfer.clone()),
                ..Facts::default()
            };
            world.deploy(&script, Some(serde_json::to_string(&facts).unwrap()));
            for (class, to) in &transfer.transferred {
                for n in world.namespaces.iter_mut() {
                    if n.script == script && &n.class == class {
                        n.script = to.clone();
                    }
                }
            }
            PhasePush::Landed
        }
        fn record(&self, _: &mut DeployLock, _: &str, _: Phase) {}
    }
    let mut world = World::default();
    world.namespace("ns-Cart", "shop-orders-staging", "Cart");
    world.deploy("core-staging", None);
    world.deploy("shop-orders-staging", None);
    let fake = Fake {
        world: Arc::new(Mutex::new(world)),
        scripts: BTreeMap::from([
            ("core".into(), "core".into()),
            ("shop-orders".into(), "shop-orders".into()),
        ]),
    };
    let pusher = Staging(fake.clone());
    let staging = MoveRun {
        environment: "staging",
        account: &fake,
        pusher: &pusher,
    };
    let mut ledger = Ledger::new();
    ledger
        .with(|w| {
            staging.open(w, one_move(), "m1", "now")?;
            assert_eq!(
                w.lock().open_moves("staging")[0].namespaces,
                BTreeMap::from([("Cart".into(), "ns-Cart".into())])
            );
            staging.receive(w, "m1")?;
            staging.commit(w, "m1")
        })
        .unwrap();
    assert_eq!(fake.pushes(), ["core:1", "shop-orders:2", "core:3"]);
    assert_eq!(
        fake.world().on("Cart"),
        [("ns-Cart".into(), "core-staging".into())]
    );
    assert!(!ledger.lock.has_open_move());
}

#[test]
fn a_receiver_that_already_has_a_namespace_for_the_class_is_refused_before_anything() {
    let fake = fake(&one_move());
    fake.world().namespace("ns-fresh", "core", "Cart");
    let mut ledger = Ledger::new();
    let stop = open(&mut ledger, &fake, one_move()).unwrap_err();
    assert_eq!(codes(&stop), ["bynk.deploy.agent_move_refused"]);
    assert_eq!(subject(&stop.refusals[0], "reason"), "receiver_has_class");
    assert!(
        stop.refusals[0].message.contains("`ns-fresh`"),
        "{}",
        stop.refusals[0].message
    );
    assert!(ledger.take().is_empty());
    assert!(fake.pushes().is_empty());
}

#[test]
fn a_move_already_done_elsewhere_says_to_pull_the_ledger() {
    let fake = fake(&one_move());
    fake.world().namespaces[0].script = "core".into();
    let mut ledger = Ledger::new();
    let stop = open(&mut ledger, &fake, one_move()).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_INTERLEAVED]);
    assert!(stop.refusals[0].message.contains("Pull `bynk.deploy.lock`"));
    assert!(ledger.take().is_empty());
}

#[test]
fn a_worker_live_at_another_moves_phase_1_says_to_pull_the_ledger() {
    let fake = fake(&one_move());
    let facts = Facts {
        v: 1,
        transfer: Some(Transfer {
            move_id: "other".into(),
            phase: 1,
            ..Transfer::default()
        }),
        ..Facts::default()
    };
    fake.world()
        .deploy("core", Some(serde_json::to_string(&facts).unwrap()));
    let mut ledger = Ledger::new();
    let stop = open(&mut ledger, &fake, one_move()).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_INTERLEAVED]);
    assert!(stop.refusals[0].message.contains("another move (`other`)"));
}

#[test]
fn a_worker_live_at_a_finished_moves_phase_3_does_not_stop_a_new_move() {
    let fake = fake(&one_move());
    let facts = Facts {
        v: 1,
        transfer: Some(Transfer {
            move_id: "earlier".into(),
            phase: 3,
            ..Transfer::default()
        }),
        ..Facts::default()
    };
    fake.world()
        .deploy("core", Some(serde_json::to_string(&facts).unwrap()));
    let mut ledger = Ledger::new();
    open(&mut ledger, &fake, one_move()).unwrap();
}

#[test]
fn a_class_with_no_namespace_on_its_source_is_refused() {
    let fake = fake(&one_move());
    fake.world().namespaces.clear();
    let mut ledger = Ledger::new();
    let stop = open(&mut ledger, &fake, one_move()).unwrap_err();
    assert_eq!(subject(&stop.refusals[0], "reason"), "no_namespace");
}

/// S3: a push cut short at each phase is pushed again by the next run, even though it landed.
#[test]
fn an_interrupted_push_at_each_phase_is_repeated_never_skipped() {
    for (worker, phase, expected, act) in [
        (
            "core",
            1,
            vec!["core:1", "core:1", "shop-orders:2", "core:3"],
        ),
        (
            "shop-orders",
            2,
            vec!["core:1", "shop-orders:2", "shop-orders:2", "core:3"],
        ),
        (
            "core",
            3,
            vec!["core:1", "shop-orders:2", "core:3", "core:3"],
        ),
    ]
    .into_iter()
    .flat_map(|(w, p, e)| {
        [Act::LandThenInterrupt, Act::Interrupt].map(|act| (w, p, e.clone(), act))
    }) {
        let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
        fake.act(worker, phase, &[act]);
        open(&mut ledger, &fake, one_move()).unwrap();
        let stop = nodes(&mut ledger, &fake).unwrap_err();
        assert!(stop.interrupted && stop.refusals.is_empty(), "{stop:?}");
        let record = ledger.record().unwrap();
        assert!(
            !record.done.contains(&worker.to_string()),
            "an interrupted push is never recorded done: {record:?}"
        );
        // The next run: the landed push is this move's own, so it is pushed again.
        nodes(&mut ledger, &fake).unwrap();
        assert_eq!(fake.pushes(), expected, "phase {phase}, {act:?}");
        assert!(!ledger.lock.has_open_move());
    }
}

#[test]
fn a_failed_push_at_each_phase_stops_and_the_next_run_repeats_it() {
    for (worker, phase, step) in [
        ("core", 1, MoveStep::Receiving),
        ("shop-orders", 2, MoveStep::Committing),
        ("core", 3, MoveStep::GoingLive),
    ] {
        let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
        fake.act(worker, phase, &[Act::Fail]);
        open(&mut ledger, &fake, one_move()).unwrap();
        let stop = nodes(&mut ledger, &fake).unwrap_err();
        assert_eq!(stop.failures.len(), 1, "{stop:?}");
        assert_eq!(ledger.record().unwrap().step, step);
        nodes(&mut ledger, &fake).unwrap();
        assert!(!ledger.lock.has_open_move());
    }
}

/// S6, on a fresh run: a commit that lands but does not move the namespace never reaches phase 3.
#[test]
fn a_commit_the_listing_does_not_show_stops_before_phase_3_and_is_repeated() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    fake.act("shop-orders", 2, &[Act::LandWithoutCommit]);
    open(&mut ledger, &fake, one_move()).unwrap();
    ledger.take();
    let stop = nodes(&mut ledger, &fake).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_COMMIT_UNCONFIRMED]);
    let e = &stop.refusals[0];
    assert_eq!(subject(e, "agents"), serde_json::json!(["Cart"]));
    assert_eq!(subject(e, "sources"), serde_json::json!(["shop-orders"]));
    assert!(
        e.message
            .contains("namespace `ns-Cart` is on `shop-orders`, not `core`"),
        "{}",
        e.message
    );
    assert_eq!(fake.pushes(), ["core:1", "shop-orders:2"], "no phase 3");
    assert_eq!(
        ledger.take(),
        [
            "receiving done=core",
            "received",
            "committing",
            "committing done=shop-orders",
            // `done` cleared for the unconfirmed source, so the next run repeats its commit.
            "committing",
        ]
    );
    nodes(&mut ledger, &fake).unwrap();
    assert_eq!(
        fake.pushes(),
        ["core:1", "shop-orders:2", "shop-orders:2", "core:3"]
    );
    assert_eq!(fake.world().on("Cart"), [("ns-Cart".into(), "core".into())]);
}

/// S6: the recorded namespace on another script is not a commit.
#[test]
fn the_recorded_namespace_on_the_wrong_script_stops_before_phase_3() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    open(&mut ledger, &fake, one_move()).unwrap();
    ledger.with(|w| run(&fake).receive(w, "m1")).unwrap();
    fake.act("shop-orders", 2, &[Act::LandWithoutCommit]);
    fake.world().namespaces[0].script = "elsewhere".into();
    let stop = ledger.with(|w| run(&fake).commit(w, "m1")).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_COMMIT_UNCONFIRMED]);
    assert!(
        stop.refusals[0]
            .message
            .contains("is on `elsewhere`, not `core`")
    );
    assert!(!fake.pushes().contains(&"core:3".to_string()));
}

/// S6: the receiver with a namespace of a new id (a class started empty there) is not a commit.
#[test]
fn the_receiving_script_with_a_new_namespace_id_stops_before_phase_3() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    open(&mut ledger, &fake, one_move()).unwrap();
    ledger.with(|w| run(&fake).receive(w, "m1")).unwrap();
    fake.act("shop-orders", 2, &[Act::LandWithoutCommit]);
    {
        let mut world = fake.world();
        world.namespaces.clear();
        world.namespace("ns-new", "core", "Cart");
    }
    let stop = ledger.with(|w| run(&fake).commit(w, "m1")).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_COMMIT_UNCONFIRMED]);
    assert!(
        stop.refusals[0]
            .message
            .contains("lists no namespace `ns-Cart` for agent `Cart` (expected on `core`)"),
        "{}",
        stop.refusals[0].message
    );
    assert!(!fake.pushes().contains(&"core:3".to_string()));
    assert_eq!(ledger.record().unwrap().step, MoveStep::Committing);
}

/// S6 on a resume, from `committed` and from `going_live`: the confirmation is made again
/// immediately before phase 3, and a failed one sends the record back to `committing`.
#[test]
fn a_resume_from_committed_or_going_live_confirms_again_before_phase_3() {
    for step in [MoveStep::Committed, MoveStep::GoingLive] {
        let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
        fake.act("core", 3, &[Act::Fail]);
        open(&mut ledger, &fake, one_move()).unwrap();
        nodes(&mut ledger, &fake).unwrap_err();
        let move_id = "m1";
        ledger
            .with(|w| w.change(|lock| lock.open_move_mut(ENV, move_id).unwrap().step = step))
            .unwrap();
        // Something outside the move puts the namespace elsewhere.
        fake.world().namespaces[0].script = "elsewhere".into();
        ledger.take();
        let stop = nodes(&mut ledger, &fake).unwrap_err();
        assert_eq!(codes(&stop), [MOVE_COMMIT_UNCONFIRMED], "{step:?}");
        assert_eq!(
            fake.pushes(),
            ["core:1", "shop-orders:2", "core:3"],
            "{step:?}: no phase 3 is pushed while the listing disagrees"
        );
        let writes = ledger.take();
        assert_eq!(
            writes.last().map(String::as_str),
            Some("committing"),
            "{writes:?}"
        );
        // The next run repeats the source's commit.
        fake.world().namespaces[0].script = "shop-orders".into();
        nodes(&mut ledger, &fake).unwrap();
        assert_eq!(
            fake.pushes(),
            [
                "core:1",
                "shop-orders:2",
                "core:3",
                "shop-orders:2",
                "core:3"
            ],
            "{step:?}"
        );
        assert!(!ledger.lock.has_open_move());
    }
}

#[test]
fn a_lost_transfer_sends_the_record_back_to_receiving_and_the_next_run_receives_again() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    fake.act("shop-orders", 2, &[Act::Lose]);
    open(&mut ledger, &fake, one_move()).unwrap();
    let stop = nodes(&mut ledger, &fake).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_TRANSFER_LOST]);
    let e = &stop.refusals[0];
    assert!(
        e.message.contains("its data is safe on `shop-orders`"),
        "{}",
        e.message
    );
    assert!(
        e.message
            .contains("phase_one_transfer_target_class_provisioned")
    );
    let record = ledger.record().unwrap();
    assert_eq!(
        (record.step, record.done.clone()),
        (MoveStep::Receiving, vec![])
    );
    nodes(&mut ledger, &fake).unwrap();
    assert_eq!(
        fake.pushes(),
        [
            "core:1",
            "shop-orders:2",
            "core:1",
            "shop-orders:2",
            "core:3"
        ]
    );
}

#[test]
fn a_worker_deployed_by_something_else_stops_the_resume_before_any_push() {
    for (worker, phase) in [("core", 1), ("shop-orders", 2), ("core", 3)] {
        let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
        fake.act(worker, phase, &[Act::Fail]);
        open(&mut ledger, &fake, one_move()).unwrap();
        nodes(&mut ledger, &fake).unwrap_err();
        let pushed = fake.pushes();
        let foreign = fake.foreign_deploy(worker);
        let stop = nodes(&mut ledger, &fake).unwrap_err();
        assert_eq!(codes(&stop), [MOVE_INTERLEAVED], "phase {phase}");
        assert_eq!(subject(&stop.refusals[0], "worker"), worker);
        assert_eq!(subject(&stop.refusals[0], "deployment"), foreign.as_str());
        assert_eq!(
            fake.pushes(),
            pushed,
            "phase {phase}: nothing more is pushed"
        );
    }
}

/// The check runs before every phase's pushes, for every Worker of the set: a receiver
/// redeployed between the two nodes stops the commit.
#[test]
fn a_receiver_redeployed_between_the_nodes_stops_the_commit() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    open(&mut ledger, &fake, one_move()).unwrap();
    ledger.with(|w| run(&fake).receive(w, "m1")).unwrap();
    fake.foreign_deploy("core");
    let stop = ledger.with(|w| run(&fake).commit(w, "m1")).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_INTERLEAVED]);
    assert_eq!(fake.pushes(), ["core:1"]);
}

/// A version of this move, but of a phase the Worker cannot be at, is not its own.
#[test]
fn a_version_of_this_move_at_another_phase_is_not_its_own() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    open(&mut ledger, &fake, one_move()).unwrap();
    let facts = Facts {
        v: 1,
        transfer: Some(Transfer {
            move_id: "m1".into(),
            phase: 2,
            ..Transfer::default()
        }),
        ..Facts::default()
    };
    fake.world()
        .deploy("core", Some(serde_json::to_string(&facts).unwrap()));
    let stop = nodes(&mut ledger, &fake).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_INTERLEAVED]);
    assert!(fake.pushes().is_empty());
}

#[test]
fn a_fan_in_commits_both_sources_at_once_and_goes_live_once() {
    let (fake, mut ledger) = (fake(&fan_in()), Ledger::new());
    open(&mut ledger, &fake, fan_in()).unwrap();
    nodes(&mut ledger, &fake).unwrap();
    let pushes = fake.pushes();
    assert_eq!(pushes.first().map(String::as_str), Some("core:1"));
    let mut commits = pushes[1..3].to_vec();
    commits.sort();
    assert_eq!(commits, ["shop-billing:2", "shop-orders:2"]);
    assert_eq!(pushes.last().map(String::as_str), Some("core:3"));
    assert_eq!(
        fake.world().on("Ledger"),
        [("ns-Ledger".into(), "core".into())]
    );
    let writes = ledger.take();
    assert!(
        writes.contains(&"committing done=shop-billing,shop-orders".to_string()),
        "{writes:?}"
    );
}

/// One of a phase's concurrent pushes failing does not cut the others short: each that lands is
/// recorded `done`, the run stops at `committing`, and the next run pushes only the one that failed.
#[test]
fn a_failed_commit_leaves_the_others_done_and_only_it_is_repeated() {
    let (fake, mut ledger) = (fake(&fan_in()), Ledger::new());
    fake.act("shop-billing", 2, &[Act::Fail]);
    open(&mut ledger, &fake, fan_in()).unwrap();
    let stop = nodes(&mut ledger, &fake).unwrap_err();
    assert_eq!(stop.failures.len(), 1);
    let record = ledger.record().unwrap();
    assert_eq!(
        (record.step, record.done.clone()),
        (MoveStep::Committing, vec!["shop-orders".to_string()])
    );
    assert!(!fake.pushes().contains(&"core:3".to_string()));
    nodes(&mut ledger, &fake).unwrap();
    let pushes = fake.pushes();
    assert_eq!(
        pushes[3..],
        ["shop-billing:2".to_string(), "core:3".to_string()]
    );
}

#[test]
fn a_fan_in_commit_left_unconfirmed_for_one_source_repeats_only_that_one() {
    let (fake, mut ledger) = (fake(&fan_in()), Ledger::new());
    fake.act("shop-billing", 2, &[Act::LandWithoutCommit]);
    open(&mut ledger, &fake, fan_in()).unwrap();
    let stop = nodes(&mut ledger, &fake).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_COMMIT_UNCONFIRMED]);
    assert_eq!(
        subject(&stop.refusals[0], "sources"),
        serde_json::json!(["shop-billing"])
    );
    let record = ledger.record().unwrap();
    assert_eq!(record.done, ["shop-orders"]);
    nodes(&mut ledger, &fake).unwrap();
    assert_eq!(
        fake.pushes()[3..],
        ["shop-billing:2".to_string(), "core:3".to_string()]
    );
}

#[test]
fn a_fan_out_receives_at_both_receivers_and_each_goes_live() {
    let (fake, mut ledger) = (fake(&fan_out()), Ledger::new());
    open(&mut ledger, &fake, fan_out()).unwrap();
    nodes(&mut ledger, &fake).unwrap();
    let pushes = fake.pushes();
    let mut receives = pushes[..2].to_vec();
    receives.sort();
    assert_eq!(receives, ["billing:1", "orders:1"]);
    assert_eq!(pushes[2], "shop:2");
    let mut lives = pushes[3..].to_vec();
    lives.sort();
    assert_eq!(lives, ["billing:3", "orders:3"]);
    assert_eq!(
        fake.world().on("Cart"),
        [("ns-Cart".into(), "orders".into())]
    );
    assert_eq!(
        fake.world().on("Ledger"),
        [("ns-Ledger".into(), "billing".into())]
    );
    let world = fake.world();
    let shop_commit = world
        .facts
        .iter()
        .filter(|((script, _), _)| script == "shop")
        .map(|(_, text)| Facts::decode(text).unwrap().transfer.unwrap())
        .next()
        .unwrap();
    assert_eq!(
        shop_commit.transferred,
        BTreeMap::from([
            ("Cart".into(), "orders".into()),
            ("Ledger".into(), "billing".into())
        ])
    );
}

/// A landed push whose `done` could not be written is not recorded, so the next run pushes it
/// again, as it would an interrupted one.
#[test]
fn a_landed_push_that_could_not_be_recorded_is_repeated() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    open(&mut ledger, &fake, one_move()).unwrap();
    ledger.take();
    // The first write after the open is core's phase 1 `done`.
    *ledger.fail_at.lock().unwrap() = Some(1);
    let stop = nodes(&mut ledger, &fake).unwrap_err();
    assert!(
        stop.failures[0]
            .message
            .contains("landed but could not be recorded"),
        "{stop:?}"
    );
    *ledger.fail_at.lock().unwrap() = None;
    // The next run reads the ledger on disk, which never said `done`.
    ledger.lock = crate::deploy::ledger::read_lock(&ledger.path).unwrap();
    assert!(ledger.record().unwrap().done.is_empty());
    nodes(&mut ledger, &fake).unwrap();
    assert_eq!(
        fake.pushes(),
        ["core:1", "core:1", "shop-orders:2", "core:3"]
    );
}

#[test]
fn a_move_id_is_eight_hex_digits() {
    let id = fresh_move_id().unwrap();
    assert_eq!(id.len(), 8);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        time::OffsetDateTime::parse(
            &now_rfc3339(),
            &time::format_description::well_known::Rfc3339
        )
        .is_ok()
    );
}

/// A set joined on a shared script can carry two classes of one name from two sources; the record
/// keys namespaces by class, so such a set is refused before anything.
#[test]
fn a_set_moving_two_classes_of_one_name_is_refused() {
    let moves = vec![
        entry("a", "Cart", "a", "b"),
        entry("c.pay", "Ledger", "c", "b"),
        entry("c.orders", "Cart", "c", "d"),
    ];
    let (fake, mut ledger) = (fake(&moves), Ledger::new());
    let stop = open(&mut ledger, &fake, moves).unwrap_err();
    assert_eq!(codes(&stop), ["bynk.deploy.agent_move_refused"]);
    assert_eq!(subject(&stop.refusals[0], "reason"), "repeated_class");
    assert_eq!(
        subject(&stop.refusals[0], "contexts"),
        serde_json::json!(["a", "c.orders"])
    );
    assert!(ledger.take().is_empty());
}

/// A deployment that lands just after this move's push, by something else, is not recorded as
/// the push's: the next check still stops on it.
#[test]
fn a_foreign_deployment_landing_after_a_push_is_not_taken_as_its_baseline() {
    struct Racing(Fake);
    impl MovePusher for Racing {
        fn push(&self, worker: &str, phase: Phase, transfer: &Transfer) -> PhasePush {
            let landed = self.0.push(worker, phase, transfer);
            self.0.foreign_deploy(worker);
            landed
        }
        fn record(&self, _: &mut DeployLock, _: &str, _: Phase) {}
    }
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    open(&mut ledger, &fake, one_move()).unwrap();
    let racing = Racing(fake.clone());
    let run = MoveRun {
        environment: ENV,
        account: &fake,
        pusher: &racing,
    };
    ledger.with(|w| run.receive(w, "m1")).unwrap();
    assert_eq!(ledger.record().unwrap().deployments["core"], "d1");
    let stop = nodes(&mut ledger, &fake).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_INTERLEAVED]);
    assert!(
        stop.refusals[0]
            .message
            .contains("make `d1` live on `core` again"),
        "{}",
        stop.refusals[0].message
    );
}

#[test]
fn a_worker_live_at_another_move_is_named_in_the_refusal() {
    let fake = fake(&fan_in());
    let facts = Facts {
        v: 1,
        transfer: Some(Transfer {
            move_id: "other".into(),
            phase: 2,
            ..Transfer::default()
        }),
        ..Facts::default()
    };
    fake.world()
        .deploy("shop-billing", Some(serde_json::to_string(&facts).unwrap()));
    let mut ledger = Ledger::new();
    let stop = open(&mut ledger, &fake, fan_in()).unwrap_err();
    assert_eq!(subject(&stop.refusals[0], "worker"), "shop-billing");
    assert_eq!(subject(&stop.refusals[0], "script"), "shop-billing");
    assert!(stop.refusals[0].subject.get("context").is_none());
}

/// A fan-in where one commit lands and the other's transfer is lost: the landed class stays
/// committed, and only the lost one goes back. Its receiver's phase 1 expects the lost class
/// alone, and only the lost source commits again (§3.4, per class).
#[test]
fn a_fan_in_with_one_lost_transfer_repeats_only_the_lost_class() {
    let (fake, mut ledger) = (fake(&fan_in()), Ledger::new());
    fake.act("shop-billing", 2, &[Act::Lose]);
    open(&mut ledger, &fake, fan_in()).unwrap();
    let stop = nodes(&mut ledger, &fake).unwrap_err();
    assert_eq!(codes(&stop), [MOVE_TRANSFER_LOST]);
    assert_eq!(subject(&stop.refusals[0], "context"), "shop.billing");
    let record = ledger.record().unwrap();
    assert_eq!(record.step, MoveStep::Receiving);
    assert_eq!(record.committed, ["Cart"]);
    assert!(record.done.is_empty(), "core receives the lost class again");
    let before = fake.pushes().len();
    nodes(&mut ledger, &fake).unwrap();
    assert_eq!(
        fake.pushes()[before..],
        [
            "core:1".to_string(),
            "shop-billing:2".to_string(),
            "core:3".to_string()
        ],
        "shop-orders, committed, is not pushed again"
    );
    // core's second phase 1 expects `Ledger` alone: `Cart` is already its.
    let world = fake.world();
    let second_receive = world
        .facts
        .iter()
        .filter(|((script, _), _)| script == "core")
        .map(|((_, version), text)| {
            let n: u32 = version.trim_start_matches('v').parse().unwrap();
            (n, Facts::decode(text).unwrap().transfer.unwrap())
        })
        .filter(|(_, t)| t.phase == 1)
        .max_by_key(|(n, _)| *n)
        .unwrap()
        .1;
    assert_eq!(
        second_receive.expecting,
        BTreeMap::from([("Ledger".into(), "shop-billing".into())])
    );
    assert_eq!(world.on("Cart"), [("ns-Cart".into(), "core".into())]);
    assert_eq!(world.on("Ledger"), [("ns-Ledger".into(), "core".into())]);
}

/// After `move_interleaved`, the way out depends on whether a commit may have landed: before it,
/// restore the recorded deployment or cancel; after it, only restore and resume, or get help.
#[test]
fn an_interleaved_stop_says_cancel_only_before_any_commit() {
    for (worker, phase, cancel) in [
        ("core", 1, true),
        ("shop-orders", 2, false),
        ("core", 3, false),
    ] {
        let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
        fake.act(worker, phase, &[Act::Fail]);
        open(&mut ledger, &fake, one_move()).unwrap();
        nodes(&mut ledger, &fake).unwrap_err();
        fake.foreign_deploy(worker);
        let stop = nodes(&mut ledger, &fake).unwrap_err();
        let message = &stop.refusals[0].message;
        assert_eq!(
            message.contains("--cancel-move shop.orders"),
            cancel,
            "phase {phase}: {message}"
        );
        assert_eq!(
            message.contains("get help"),
            !cancel,
            "phase {phase}: {message}"
        );
        assert!(message.contains("wrangler rollback"), "{message}");
    }
}

/// D8: before any commit, `--cancel-move` pushes each receiver without its pending transfers and
/// closes the record; the data never left the source.
#[test]
fn a_move_is_cancelled_before_its_commit_and_its_record_closed() {
    let (fake, mut ledger) = (fake(&fan_out()), Ledger::new());
    open(&mut ledger, &fake, fan_out()).unwrap();
    ledger.with(|w| run(&fake).receive(w, "m1")).unwrap();
    ledger.with(|w| run(&fake).cancel(w, "m1")).unwrap();
    let mut cancels = fake.pushes()[2..].to_vec();
    cancels.sort();
    assert_eq!(cancels, ["billing:4", "orders:4"]);
    assert!(!ledger.lock.has_open_move());
    assert_eq!(fake.world().on("Cart"), [("ns-Cart".into(), "shop".into())]);
    // A later move of the same contexts is not stopped by the cancelled one's versions.
    open(&mut ledger, &fake, fan_out()).unwrap();
}

/// D8: from the commit on, a cancel is refused and nothing is pushed.
#[test]
fn a_cancel_after_a_commit_may_have_landed_is_refused() {
    let (fake, mut ledger) = (fake(&one_move()), Ledger::new());
    fake.act("core", 3, &[Act::Fail]);
    open(&mut ledger, &fake, one_move()).unwrap();
    nodes(&mut ledger, &fake).unwrap_err();
    let pushed = fake.pushes();
    let stop = ledger.with(|w| run(&fake).cancel(w, "m1")).unwrap_err();
    assert_eq!(codes(&stop), ["bynk.deploy.move_open"]);
    assert!(stop.refusals[0].message.contains("The only way is forward"));
    assert_eq!(fake.pushes(), pushed);
    assert!(ledger.lock.has_open_move());
}

/// A cancel cut short leaves the cancelled receiver not `done`: a resume receives it again, and
/// its cancel push is the move's own.
#[test]
fn a_cancel_cut_short_resumes_by_receiving_again() {
    let (fake, mut ledger) = (fake(&fan_out()), Ledger::new());
    open(&mut ledger, &fake, fan_out()).unwrap();
    ledger.with(|w| run(&fake).receive(w, "m1")).unwrap();
    // `received` → back to `receiving` is not a step a run takes; a cancel happens before the
    // commit, so put the record where a cancel finds it mid-phase 1.
    ledger
        .with(|w| {
            w.change(|lock| {
                let r = lock.open_move_mut(ENV, "m1").unwrap();
                r.step = MoveStep::Receiving;
                r.done = vec!["billing".into(), "orders".into()];
            })
        })
        .unwrap();
    fake.act("orders", 4, &[Act::Fail]);
    let stop = ledger.with(|w| run(&fake).cancel(w, "m1")).unwrap_err();
    assert_eq!(stop.failures.len(), 1);
    assert_eq!(ledger.record().unwrap().done, ["orders"]);
    nodes(&mut ledger, &fake).unwrap();
    assert!(!ledger.lock.has_open_move());
}

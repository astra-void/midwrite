//! A coverage-driven generator of scenarios for testing reductions
//! (Phase 10), and what it needs around it: dynamic feature observation, a
//! minimizer and metamorphic transformations.
//!
//! Why it exists. The Phase 9 random generator drew two or three actors on
//! one table with no pools, no shared processes and no tasks, so 700 passing
//! seeds said nothing about the transitions that were missing from it; the
//! pool hand-over defect lived exactly there. This generator picks a
//! *profile* per seed; a profile fixes which resources the actors share and
//! keeps everything else private, so that the one dependency a profile
//! exercises is not masked by others (a dependency only matters where
//! independence would otherwise hold). The profiles:
//!
//! * `Rows`: private tables plus one shared table, row/predicate/unique
//!   conflicts.
//! * `Pool`: actors of one process share a bounded connection pool.
//! * `Tasks`: concurrent tasks of one actor (`spawn`/`join`).
//! * `App`: actors of one process share in-process variables.
//! * `Seq`: autocommit inserts that draw ids from one sequence.
//! * `Faults`: crashes, retries, lost acknowledgments (in flight), with
//!   private data so the shared fault budget is the only coupling.
//! * `Locks`: `FOR UPDATE [SKIP LOCKED]` with private external channels.
//! * `Serializable`: predicate reads and SERIALIZABLE bookkeeping.
//! * `Mixed`: several of the above at once.
//!
//! What a seed generated is recorded as *features* (`pool`, `unique`, ...);
//! what the model can *do* is observed by walking its state space
//! ([`observe`]): the kinds of transitions, a waiting claimant that gets a
//! connection, a crash with an operation in flight, a statement that resumes
//! after a lock wait. A campaign reports how often each feature and each
//! combination in [`COMBINATIONS`] occurred, so that a pass over many trivial
//! models is not mistaken for coverage.
#![allow(dead_code)]

use std::collections::BTreeSet;

use super::scenarios_gen::Rng;
use midwrite_model::model::Model;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Profile {
    Rows,
    Pool,
    Tasks,
    App,
    Seq,
    Faults,
    Locks,
    Serializable,
    Mixed,
}

pub const PROFILES: [Profile; 9] = [
    Profile::Rows,
    Profile::Pool,
    Profile::Tasks,
    Profile::App,
    Profile::Seq,
    Profile::Faults,
    Profile::Locks,
    Profile::Serializable,
    Profile::Mixed,
];

pub struct Generated {
    pub seed: u64,
    pub profile: Profile,
    pub doc: Value,
    /// What the generator put into the scenario.
    pub features: BTreeSet<String>,
}

/// What the generator may use in one scenario.
#[derive(Clone, Default)]
struct Knobs {
    actors: (u64, u64),
    ops: (u64, u64),
    private_tables: bool,
    shared_table: bool,
    /// Percent of statements that use a private table.
    p_private: u64,
    lock: bool,
    skip_locked: bool,
    insert: bool,
    delete: bool,
    count: bool,
    rmw: bool,
    seq: bool,
    ext: bool,
    ext_private: bool,
    app: bool,
    spawn: bool,
    unique_on_w: bool,
    tx: u64,
    pool: Option<(u16, u16)>,
    one_process: bool,
    crash: u64,
    retry: u64,
    in_flight: u64,
    ser: u64,
    /// The first two actors lock two rows in opposite orders.
    lock_order: bool,
}

struct Gen {
    rng: Rng,
    feats: BTreeSet<String>,
    var: usize,
}

impl Gen {
    fn f(&mut self, name: &str) {
        self.feats.insert(name.to_owned());
    }

    fn v(&mut self) -> usize {
        self.var += 1;
        self.var
    }

    fn pick<T: Clone>(&mut self, xs: &[T]) -> T {
        xs[self.rng.below(xs.len() as u64) as usize].clone()
    }

    fn between(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.rng.below(hi - lo + 1)
    }
}

const ISOS: [&str; 3] = ["read-committed", "repeatable-read", "serializable"];

/// One statement against `table`; may add a few lines.
fn statement(g: &mut Gen, k: &Knobs, table: &str, actor: usize, out: &mut String) {
    let n = g.v();
    let key = g.between(1, 3);
    // The palette, restricted by the knobs.
    let mut kinds: Vec<&str> = vec!["read", "inc", "guarded"];
    if k.rmw {
        kinds.push("rmw");
    }
    if k.count {
        kinds.push("count");
    }
    if k.insert {
        kinds.push("insert");
    }
    if k.delete {
        kinds.push("delete");
    }
    if k.lock {
        kinds.push("lock");
    }
    if k.skip_locked {
        kinds.push("skip");
    }
    let kind = g.pick(&kinds);
    match kind {
        "read" => {
            out.push_str(&format!("let r{n} = select {table} where @id == {key}\n"));
            g.f("stmt.read");
        }
        "rmw" => {
            let d = g.between(1, 3);
            out.push_str(&format!(
                "let r{n} = select {table} where @id == {key}\nif r{n} != null {{\n  update {table} where @id == {key} set @v = r{n}.v + {d}\n}}\n"
            ));
            g.f("stmt.read-modify-write");
        }
        "inc" => {
            let d = g.between(1, 3);
            out.push_str(&format!(
                "update {table} where @id == {key} set @v = @v + {d} into a{n}\n"
            ));
            g.f("stmt.update");
        }
        "guarded" => {
            let c = g.between(0, 2);
            out.push_str(&format!(
                "update {table} where @id == {key} && @v > {c} set @v = @v - 1 into u{n}\n"
            ));
            g.f("stmt.guarded-update");
        }
        "count" => {
            let c = g.between(0, 3);
            out.push_str(&format!("let c{n} = count {table} where @v >= {c}\n"));
            g.f("stmt.predicate-read");
        }
        "insert" => {
            let id = g.between(1, 5);
            let v = g.between(0, 3);
            let w = g.between(10, 15);
            let clause = if k.unique_on_w && g.rng.chance(70) {
                g.f("unique");
                " on conflict do nothing"
            } else {
                ""
            };
            out.push_str(&format!(
                "insert {table} set @id = {id}, @v = {v}, @w = {w}{clause} into i{n}\n"
            ));
            g.f("stmt.insert");
        }
        "delete" => {
            out.push_str(&format!("delete {table} where @id == {key} into d{n}\n"));
            g.f("stmt.delete");
        }
        "lock" => {
            out.push_str(&format!(
                "let l{n} = select {table} where @id == {key} for update\n"
            ));
            g.f("lock.for-update");
        }
        "skip" => {
            out.push_str(&format!(
                "let l{n} = select {table} where @id == {key} for update skip locked\nif l{n} != null {{\n  update {table} where @id == {key} set @v = @v + 1\n}}\n"
            ));
            g.f("lock.skip-locked");
        }
        _ => unreachable!(),
    }
    // Side effects and failures.
    if k.ext && g.rng.chance(25) {
        let ch = if k.ext_private {
            format!("c{actor}")
        } else {
            "ch".to_owned()
        };
        let key = g.between(1, 2);
        out.push_str(&format!("external {ch} key {key} set n = {n}\n"));
        g.f(if k.ext_private {
            "external.private"
        } else {
            "external.shared"
        });
    }
    if g.rng.chance(8) {
        out.push_str(&format!(
            "if {} == 1 {{\n  fail \"generated failure\"\n}}\n",
            g.rng.below(2)
        ));
        g.f("failure");
    }
}

fn table_for(g: &mut Gen, k: &Knobs, actor: usize) -> String {
    let private = format!("p{actor}");
    if k.private_tables && (!k.shared_table || g.rng.chance(k.p_private)) {
        private
    } else {
        "t".to_owned()
    }
}

fn actor_program(g: &mut Gen, k: &Knobs, actor: usize, tx_iso: Option<&'static str>) -> String {
    let mut out = String::new();
    if k.lock_order && actor < 2 {
        // Two transactions that lock the same two rows in opposite orders.
        let (a, b) = if actor == 0 { (1, 2) } else { (2, 1) };
        out.push_str(&format!(
            "begin\nlet la = select t where @id == {a} for update\nlet lb = select t where @id == {b} for update\ncommit\n"
        ));
        g.f("lock.opposite-order");
        return out;
    }
    let in_tx = g.rng.chance(k.tx);
    if in_tx {
        match tx_iso {
            Some(i) => out.push_str(&format!("begin isolation {i}\n")),
            None => out.push_str("begin\n"),
        }
        g.f("tx");
    } else {
        g.f("autocommit");
    }
    let ops = g.between(k.ops.0, k.ops.1) as usize;
    let mut spawned = false;
    for _ in 0..ops {
        if k.spawn && !spawned && g.rng.chance(45) {
            // Concurrent tasks of this actor; their statements share the
            // actor's locals and its transaction.
            spawned = true;
            out.push_str("spawn {\n");
            for t in 0..2 {
                let table = table_for(g, k, actor);
                statement(g, k, &table, actor, &mut out);
                if g.rng.chance(40) {
                    // The same local name in both children: a conflict
                    // through shared locals.
                    out.push_str(&format!("let shared = count {table} where @v >= 0\n"));
                }
                if t == 0 {
                    out.push_str("} {\n");
                }
            }
            out.push_str("}\njoin\n");
            g.f("tasks.spawn-join");
            continue;
        }
        if k.app && g.rng.chance(40) {
            let n = g.v();
            out.push_str(&format!("read_app n into c{n}\nwrite_app n = c{n} + 1\n"));
            g.f("app.read-write");
            continue;
        }
        if k.seq && g.rng.chance(50) {
            let n = g.v();
            out.push_str(&format!("insert log set @who = {actor} into s{n}\n"));
            g.f("seq.insert");
            continue;
        }
        let table = table_for(g, k, actor);
        statement(g, k, &table, actor, &mut out);
    }
    if in_tx {
        if g.rng.chance(15) {
            out.push_str("rollback\n");
            g.f("rollback");
        } else {
            out.push_str("commit\n");
        }
    }
    out
}

fn build(seed: u64, profile: Profile, k: &Knobs, g: &mut Gen) -> Value {
    let n_actors = g.between(k.actors.0, k.actors.1) as usize;
    let iso = if g.rng.chance(k.ser) {
        "serializable"
    } else {
        g.pick(&ISOS[..2])
    };
    if iso != "read-committed" {
        g.f(&format!("isolation.{iso}"));
    }
    let mut tables = Vec::new();
    let mut initial = serde_json::Map::new();
    let mut table_defs: Vec<String> = Vec::new();
    if k.shared_table {
        table_defs.push("t".into());
    }
    if k.private_tables {
        for i in 0..n_actors {
            table_defs.push(format!("p{i}"));
        }
    }
    for (ti, name) in table_defs.iter().enumerate() {
        let mut def = json!({
            "name": name,
            "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}, {"name":"w","type":"int"}],
            "primaryKey": ["id"]
        });
        if k.unique_on_w {
            def["unique"] = json!([["w"]]);
        }
        tables.push(def);
        let base = 10 + 10 * ti as i64;
        initial.insert(
            name.clone(),
            json!([
                {"id": 1, "v": 1, "w": base + 1}, {"id": 2, "v": 2, "w": base + 2}, {"id": 3, "v": 0, "w": base + 3}
            ]),
        );
    }
    if k.seq {
        tables.push(json!({
            "name": "log",
            "columns": [
                {"name":"id","type":"int","notNull":true,"default":{"sequence":{}}},
                {"name":"who","type":"int"}
            ],
            "primaryKey": ["id"]
        }));
        initial.insert("log".into(), json!([]));
        g.f("seq");
    }
    let (pooled, pool_n) = match k.pool {
        Some((lo, hi)) => (true, g.between(lo as u64, hi as u64) as u16),
        None => (false, 0),
    };
    let one_process = k.one_process || pooled;
    let crashes = u64::from(g.rng.chance(k.crash));
    let retries = u64::from(g.rng.chance(k.retry));
    let in_flight = u64::from(g.rng.chance(k.in_flight));
    let mut actors = Vec::new();
    for i in 0..n_actors {
        let tx_iso = if g.rng.chance(15) {
            Some(g.pick(&ISOS))
        } else {
            None
        };
        let program = actor_program(g, k, i, tx_iso);
        let mut a = json!({
            "name": format!("A{i}"),
            "function": "work",
            "program": program,
            "arguments": {}
        });
        // The first two actors of a pooled or shared-process scenario share
        // the process; a third one may be outside it.
        if one_process && (i < 2 || g.rng.chance(70)) {
            a["process"] = json!("P");
        }
        if retries == 1 || g.rng.chance(k.retry / 2) {
            let triggers = ["onFailure", "onCrash", "onFailureOrCrash", "atLeastOnce"];
            a["retry"] = json!({ "trigger": g.pick(&triggers) });
            g.f("retry");
        }
        actors.push(a);
    }
    if one_process {
        g.f("process.shared");
    }
    if pooled {
        g.f("pool");
        g.f(&format!("pool.{pool_n}"));
    }
    if crashes == 1 {
        g.f("fault.crash");
    }
    if retries == 1 {
        g.f("fault.retry-budget");
    }
    if in_flight == 1 {
        g.f("fault.in-flight");
    }
    let mut doc = json!({
        "schemaVersion": 1,
        "name": format!("adv-{seed}-{profile:?}"),
        "database": { "isolation": iso, "tables": tables, "initialState": initial },
        "channels": {
            "ch": { "dedup": if g.rng.chance(50) { "byKey" } else { "none" } },
            "c0": { "dedup": "none" }, "c1": { "dedup": "none" }, "c2": { "dedup": "none" }
        },
        "actors": actors,
        "bounds": {
            "maxSteps": 60, "maxStates": 300000,
            "maxCrashes": crashes, "maxRetries": retries, "maxInFlight": in_flight
        },
        "invariants": invariants(k, g),
    });
    if k.app {
        doc["app"] = json!({"n": 0});
        g.f("app");
    }
    if pooled {
        doc["pools"] = json!({"P": pool_n});
    }
    doc
}

/// Final invariants only: a reduction is refused with an `always` one.
fn invariants(k: &Knobs, g: &mut Gen) -> Value {
    let mut inv = vec![];
    if k.ext {
        let ch = if k.ext_private { "c0" } else { "ch" };
        inv.push(json!({"name": "few-effects", "when": "final",
            "assert": {"le": [{"externals": {"channel": ch}}, 1]}}));
    }
    if k.shared_table {
        inv.push(json!({"name": "rows-kept", "when": "final",
            "assert": {"ge": [{"count": {"table": "t", "where": {}}}, 2]}}));
        inv.push(json!({"name": "sum-bound", "when": "final",
            "assert": {"le": [{"sum": {"table": "t", "where": {}, "column": "v"}}, 8]}}));
    }
    if k.seq {
        inv.push(json!({"name": "log-small", "when": "final",
            "assert": {"le": [{"count": {"table": "log", "where": {}}}, 3]}}));
    }
    if k.app {
        // Lost updates on the shared counter are what this checks for.
        inv.push(json!({"name": "app-count", "when": "final",
            "assert": {"ge": [{"count": {"table": if k.shared_table { "t" } else { "p0" }, "where": {}}}, 0]}}));
    }
    if inv.is_empty() {
        inv.push(json!({"name": "trivial", "when": "final",
            "assert": {"ge": [{"count": {"table": "p0", "where": {}}}, 0]}}));
    }
    let _ = g;
    Value::Array(inv)
}

fn knobs(profile: Profile, g: &mut Gen) -> Knobs {
    let base = Knobs {
        actors: (2, 3),
        ops: (1, 3),
        tx: 60,
        p_private: 60,
        ..Knobs::default()
    };
    match profile {
        Profile::Rows => Knobs {
            private_tables: true,
            shared_table: true,
            lock: g.rng.chance(30),
            insert: true,
            delete: g.rng.chance(50),
            count: true,
            rmw: true,
            ext: g.rng.chance(40),
            ext_private: g.rng.chance(50),
            unique_on_w: g.rng.chance(60),
            ser: 25,
            crash: 15,
            retry: 15,
            ..base
        },
        Profile::Pool => Knobs {
            actors: (2, 3),
            ops: (1, 2),
            private_tables: true,
            shared_table: g.rng.chance(40),
            p_private: 80,
            insert: g.rng.chance(40),
            rmw: false,
            pool: Some((1, 2)),
            one_process: true,
            spawn: g.rng.chance(35),
            crash: 45,
            retry: 25,
            in_flight: 35,
            ..base
        },
        Profile::Tasks => Knobs {
            actors: (1, 2),
            ops: (1, 2),
            private_tables: g.rng.chance(50),
            shared_table: true,
            p_private: 40,
            spawn: true,
            rmw: true,
            count: true,
            crash: 15,
            ..base
        },
        Profile::App => Knobs {
            ops: (1, 3),
            private_tables: true,
            shared_table: false,
            app: true,
            one_process: true,
            ext: true,
            ext_private: true,
            crash: 50,
            retry: 30,
            ..base
        },
        Profile::Seq => Knobs {
            ops: (1, 2),
            private_tables: true,
            shared_table: false,
            seq: true,
            tx: 30,
            crash: 10,
            ..base
        },
        Profile::Faults => Knobs {
            ops: (2, 3),
            private_tables: true,
            shared_table: g.rng.chance(30),
            p_private: 85,
            insert: true,
            rmw: true,
            unique_on_w: true,
            tx: 85,
            ext: true,
            ext_private: true,
            crash: 55,
            retry: 55,
            in_flight: 85,
            one_process: g.rng.chance(25),
            ..base
        },
        Profile::Locks => Knobs {
            ops: (1, 2),
            private_tables: false,
            shared_table: true,
            lock: true,
            skip_locked: true,
            ext: true,
            ext_private: true,
            tx: 90,
            ser: 0,
            lock_order: g.rng.chance(45),
            ..base
        },
        Profile::Serializable => Knobs {
            ops: (1, 3),
            private_tables: true,
            shared_table: true,
            p_private: 50,
            count: true,
            insert: true,
            rmw: true,
            ser: 100,
            tx: 90,
            ..base
        },
        Profile::Mixed => Knobs {
            actors: (2, 3),
            ops: (1, 2),
            private_tables: true,
            shared_table: true,
            p_private: 55,
            lock: g.rng.chance(50),
            skip_locked: g.rng.chance(30),
            insert: true,
            delete: g.rng.chance(40),
            count: g.rng.chance(60),
            rmw: true,
            seq: g.rng.chance(35),
            ext: true,
            ext_private: g.rng.chance(50),
            app: g.rng.chance(30),
            spawn: g.rng.chance(30),
            unique_on_w: g.rng.chance(50),
            pool: if g.rng.chance(35) { Some((1, 2)) } else { None },
            one_process: g.rng.chance(40),
            ser: 20,
            crash: 35,
            retry: 30,
            in_flight: 45,
            ..base
        },
    }
}

pub fn generate(seed: u64) -> Generated {
    generate_profile(seed, PROFILES[(seed % PROFILES.len() as u64) as usize])
}

pub fn generate_profile(seed: u64, profile: Profile) -> Generated {
    let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) ^ 0xD1B54A32D192ED03 | 1);
    for _ in 0..6 {
        rng.next();
    }
    let mut g = Gen {
        rng,
        feats: BTreeSet::new(),
        var: 0,
    };
    g.f(&format!("profile.{profile:?}"));
    let k = knobs(profile, &mut g);
    let doc = build(seed, profile, &k, &mut g);
    Generated {
        seed,
        profile,
        doc,
        features: g.feats,
    }
}

// ---------------------------------------------------------------------------
// Dynamic features
// ---------------------------------------------------------------------------

/// What a walk over the model's state space (up to `limit` states) saw.
pub fn observe(model: &Model, limit: usize) -> BTreeSet<&'static str> {
    use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
    use midwrite_model::state::{Lease, TKind};
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    let mut keys: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    let init = initial_state(model);
    keys.insert(init.canonical(model));
    let mut stack = vec![init];
    while let Some(s) = stack.pop() {
        if keys.len() > limit {
            break;
        }
        let waiting = |st: &midwrite_model::state::State| {
            st.actors
                .iter()
                .flat_map(|a| a.threads.iter())
                .filter(|t| t.lease == Lease::Waiting)
                .count()
        };
        if waiting(&s) > 0 {
            seen.insert("state.pool-waiter");
        }
        if s.pools.iter().any(|p| p.queue.len() > 1) {
            seen.insert("state.pool-queue>1");
        }
        if s.actors
            .iter()
            .flat_map(|a| a.threads.iter())
            .any(|t| t.pending.as_ref().is_some_and(|p| p.blocked_on.is_some()))
        {
            seen.insert("state.lock-wait");
        }
        if !s.orphans.is_empty() {
            seen.insert("state.orphan-pending");
        }
        if s.txns.values().filter(|t| t.committed.is_none()).count() > 1 {
            seen.insert("state.concurrent-txns");
        }
        if s.externals.iter().any(|e| e.deduplicated) {
            seen.insert("state.external-deduplicated");
        }
        for t in enabled(model, &s) {
            seen.insert(match t.kind {
                TKind::Step => "tk.step",
                TKind::Resume => "tk.resume",
                TKind::Crash => "tk.crash",
                TKind::Retry => "tk.retry",
                TKind::Deadlock => "tk.deadlock",
                TKind::LostCommitted => "tk.lost-committed",
                TKind::LostNotCommitted => "tk.lost-not-committed",
                TKind::CrashInFlight => "tk.crash-in-flight",
                TKind::ServerComplete => "tk.server-complete",
                TKind::Replicate => "tk.replicate",
            });
            let Ok(a) = apply(model, &s, t, &mut Recorder::off()) else {
                continue;
            };
            // A waiting claimant that gets a connection by a release.
            let was = waiting(&s);
            let now = waiting(&a.state);
            if was > now && t.kind != TKind::Crash && t.kind != TKind::CrashInFlight {
                seen.insert("tr.pool-release-hands-over");
            }
            if t.kind == TKind::Resume {
                seen.insert("tr.lock-release-resumes-statement");
            }
            if (t.kind == TKind::Crash || t.kind == TKind::CrashInFlight) && !s.orphans.is_empty() {
                seen.insert("tr.crash-with-pending-server-op");
            }
            if t.kind == TKind::ServerComplete
                && s.actors.iter().any(|x| x.completions > 0 || x.attempt > 0)
            {
                seen.insert("tr.server-complete-after-restart");
            }
            let key = a.state.canonical(model);
            if keys.insert(key) && a.state.steps < model.bounds.max_steps {
                stack.push(a.state);
            }
        }
    }
    seen
}

/// Feature combinations a campaign must reach, each as (name, required
/// features: generator features, then observed ones). A combination counts for
/// a scenario when all of its members are present, which is weaker than "in the
/// same state" but is what a campaign can verify cheaply.
pub const COMBINATIONS: [(&str, &[&str]); 14] = [
    (
        "pool release + waiting claimant",
        &["pool", "tr.pool-release-hands-over"],
    ),
    ("pool + crash", &["pool", "fault.crash", "tk.crash"]),
    (
        "crash + pending COMMIT",
        &["fault.in-flight", "tr.crash-with-pending-server-op"],
    ),
    ("retry + unique constraint", &["retry", "unique"]),
    (
        "lock release + statement resumes",
        &["tr.lock-release-resumes-statement"],
    ),
    (
        "ServerComplete + restart",
        &["tr.server-complete-after-restart"],
    ),
    (
        "lost acknowledgment",
        &["tk.lost-committed", "tk.lost-not-committed"],
    ),
    (
        "shared process + crash (no pool)",
        &["process.shared", "tk.crash"],
    ),
    (
        "tasks of one actor",
        &["tasks.spawn-join", "state.concurrent-txns"],
    ),
    (
        "app variable + shared process",
        &["app.read-write", "process.shared"],
    ),
    ("sequence inserts", &["seq.insert"]),
    ("skip locked", &["lock.skip-locked"]),
    ("deadlock reachable", &["tk.deadlock"]),
    (
        "serializable predicate reads",
        &["isolation.serializable", "stmt.predicate-read"],
    ),
];

/// Combinations (by index into [`COMBINATIONS`]) a scenario covers.
pub fn combinations_of(
    generated: &BTreeSet<String>,
    observed: &BTreeSet<&'static str>,
) -> Vec<usize> {
    COMBINATIONS
        .iter()
        .enumerate()
        .filter(|(_, (_, need))| {
            need.iter()
                .all(|f| generated.contains(*f) || observed.contains(f))
        })
        .map(|(i, _)| i)
        .collect()
}

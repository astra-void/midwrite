//! The PostgreSQL differential matrix: workload × isolation level ×
//! schedule × constraint configuration. For each configuration *every*
//! schedule of the model (up to a cap, sampled evenly beyond it) is replayed
//! against a real PostgreSQL in lockstep and compared with the model.

mod common;

use commitrift_model::model::Model;
use commitrift_model::oracle::{PgEnv, run_schedule};
use commitrift_model::witness::enumerate_schedules;
use common::*;
use serde_json::json;

fn env_or_skip() -> Option<PgEnv> {
    match PgEnv::start() {
        Ok(env) => Some(env),
        Err(reason) => {
            assert!(
                std::env::var_os("COMMITRIFT_REQUIRE_REPLAY").is_none(),
                "PostgreSQL is required but unavailable: {reason}"
            );
            eprintln!("skipping: {reason}");
            None
        }
    }
}

/// Attempts for a schedule whose deadlock victim PostgreSQL chose differently
/// from the one the schedule steers to.
const VICTIM_ATTEMPTS: usize = 3;

struct Outcome {
    schedules: usize,
    total: usize,
    mismatches: Vec<String>,
}

fn differential(env: &mut PgEnv, label: &str, model: &Model, max_runs: usize) -> Outcome {
    let (all, truncated) = enumerate_schedules(model, 20_000);
    assert!(!all.is_empty(), "{label}: no schedules");
    let total = all.len();
    let picked: Vec<_> = if total <= max_runs {
        all
    } else {
        let step = total as f64 / max_runs as f64;
        (0..max_runs)
            .map(|i| all[(i as f64 * step) as usize].clone())
            .collect()
    };
    let db = env
        .database(model)
        .unwrap_or_else(|e| panic!("{label}: {e}"));
    let mut mismatches = Vec::new();
    let mut victim_retries = 0usize;
    for schedule in &picked {
        // Which member of a deadlock cycle PostgreSQL aborts depends on
        // which backend's `deadlock_timeout` expires first, and the oracle
        // steers that by timing (`DEADLOCK_TIMEOUT_MS`). A load spike can make
        // PostgreSQL choose the other member: a valid server behavior, not a
        // model disagreement, but not what this schedule asks for. Only that
        // observation is retried, a bounded number of times, and every retry
        // is counted and printed. Any other disagreement fails at once, and
        // so does a victim that is still wrong after the last attempt.
        let mut attempt = 1;
        let result = loop {
            let r = run_schedule(env, &db, model, schedule);
            let victim_differs = matches!(&r, Ok(run) if !run.agree
                && run
                    .observations
                    .iter()
                    .filter(|o| !o.agree)
                    .all(|o| o.sql == "(deadlock detection)"));
            if victim_differs && attempt < VICTIM_ATTEMPTS {
                attempt += 1;
                victim_retries += 1;
                continue;
            }
            break r;
        };
        match result {
            Ok(run) if run.agree => {}
            Ok(run) => {
                let bad: Vec<String> = run
                    .observations
                    .iter()
                    .filter(|o| !o.agree)
                    .map(|o| {
                        format!(
                            "#{} {}: {} → predicted {} observed {}",
                            o.transition, o.actor, o.sql, o.predicted, o.observed
                        )
                    })
                    .collect();
                mismatches.push(format!(
                    "{label} schedule {:?}\n   {}{}",
                    schedule
                        .iter()
                        .map(|t| format!("{:?}{}.{}", t.kind, t.actor, t.thread))
                        .collect::<Vec<_>>(),
                    bad.join("\n   "),
                    if run.tables_agree {
                        String::new()
                    } else {
                        format!(
                            "\n   tables: model {:?} postgres {:?}",
                            run.model_tables, run.postgres_tables
                        )
                    }
                ));
            }
            Err(e) => mismatches.push(format!("{label}: infrastructure error: {e}")),
        }
    }
    eprintln!(
        "{label}: {} of {} schedule(s) replayed{} ({victim_retries} retried after PostgreSQL chose another deadlock victim), {} mismatch(es)",
        picked.len(),
        total,
        if truncated {
            " (enumeration truncated)"
        } else {
            ""
        },
        mismatches.len()
    );
    Outcome {
        schedules: picked.len(),
        total,
        mismatches,
    }
}

fn run_matrix(configs: Vec<(String, Model)>, max_runs: usize) {
    let Some(mut env) = env_or_skip() else { return };
    eprintln!("PostgreSQL: {}", env.version());
    let mut bad = Vec::new();
    let mut runs = 0;
    for (label, model) in &configs {
        let o = differential(&mut env, label, model, max_runs);
        runs += o.schedules;
        let _ = o.total;
        bad.extend(o.mismatches);
    }
    eprintln!("{runs} schedule replay(s) in total");
    assert!(
        bad.is_empty(),
        "{} disagreement(s):\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn lost_update_workloads() {
    let mut configs = Vec::new();
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        configs.push((
            format!("purchase-autocommit/{iso}"),
            purchase(iso, PURCHASE_AUTOCOMMIT, json!({})),
        ));
        configs.push((
            format!("purchase-tx/{iso}"),
            purchase(iso, PURCHASE_TX, json!({})),
        ));
        configs.push((
            format!("purchase-for-update/{iso}"),
            purchase(iso, PURCHASE_FOR_UPDATE, json!({})),
        ));
        configs.push((
            format!("purchase-atomic/{iso}"),
            purchase(iso, PURCHASE_ATOMIC, json!({})),
        ));
        configs.push((
            format!("purchase-atomic-tx/{iso}"),
            purchase(iso, PURCHASE_ATOMIC_TX, json!({})),
        ));
    }
    run_matrix(configs, 100_000);
}

#[test]
fn write_skew_workloads() {
    let configs = ["read-committed", "repeatable-read", "serializable"]
        .iter()
        .map(|iso| (format!("write-skew/{iso}"), write_skew(iso, json!({}))))
        .collect();
    run_matrix(configs, 100_000);
}

#[test]
fn check_then_act_workloads() {
    let mut configs = Vec::new();
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        configs.push((
            format!("redeem-no-constraint/{iso}"),
            redeem(iso, false, json!({})),
        ));
        configs.push((format!("redeem-unique/{iso}"), redeem(iso, true, json!({}))));
    }
    run_matrix(configs, 100_000);
}

#[test]
fn deadlock_workloads() {
    let configs = ["read-committed", "repeatable-read"]
        .iter()
        .map(|iso| (format!("deadlock/{iso}"), deadlock(iso, json!({}))))
        .collect();
    run_matrix(configs, 100_000);
}

#[test]
fn insert_conflict_and_unique_workloads() {
    let mut configs = Vec::new();
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        configs.push((
            format!("on-conflict-do-nothing/{iso}"),
            insert_race(iso, INSERT_DO_NOTHING, INSERT_DO_NOTHING, true),
        ));
        configs.push((
            format!("on-conflict-after-read/{iso}"),
            insert_race(
                iso,
                INSERT_DO_NOTHING_AFTER_READ,
                INSERT_DO_NOTHING_AFTER_READ,
                true,
            ),
        ));
        configs.push((
            format!("insert-rollback-vs-commit/{iso}"),
            insert_race(iso, INSERT_THEN_ROLLBACK, INSERT_THEN_COMMIT, true),
        ));
        configs.push((
            format!("insert-vs-insert-unique/{iso}"),
            insert_race(iso, INSERT_THEN_COMMIT, INSERT_THEN_COMMIT, true),
        ));
    }
    run_matrix(configs, 100_000);
}

#[test]
fn constraint_delete_and_locking_workloads() {
    let mut configs = Vec::new();
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        configs.push((format!("check-constraint/{iso}"), check_constraint(iso)));
        configs.push((format!("delete-vs-update/{iso}"), delete_vs_update(iso)));
        configs.push((
            format!("claim-skip-locked/{iso}"),
            jobs(iso, CLAIM_SKIP_LOCKED, 1),
        ));
        configs.push((
            format!("claim-skip-locked-2/{iso}"),
            jobs(iso, CLAIM_SKIP_LOCKED, 2),
        ));
        configs.push((format!("claim-nowait/{iso}"), jobs(iso, CLAIM_NOWAIT, 1)));
        configs.push((format!("claim-plain/{iso}"), jobs(iso, CLAIM_PLAIN, 1)));
    }
    run_matrix(configs, 100_000);
}

#[test]
fn crash_retry_and_task_workloads() {
    let configs = vec![
        (
            "crash-after-commit".to_owned(),
            crash_after_commit(1, None, 0),
        ),
        (
            "crash-then-client-retry".to_owned(),
            crash_after_commit(1, Some("onCrash"), 1),
        ),
        ("tx-retry/repeatable-read".to_owned(), tx_retry(2)),
        ("concurrent-tasks".to_owned(), concurrent_tasks()),
    ];
    run_matrix(configs, 100_000);
}

/// A documented, expected difference: PostgreSQL's SSI tracks reads with
/// predicate locks whose granularity depends on the plan (a sequential scan
/// of a small table locks the whole relation), so it may abort transactions
/// whose *logical* read and write sets do not conflict. The model checks the
/// logical serialization graph and is therefore more permissive. Every
/// disagreement must go in that direction: PostgreSQL raised 40001 where the
/// model predicted success, never the reverse.
#[test]
fn ssi_granularity_differences_only_go_one_way() {
    let Some(mut env) = env_or_skip() else { return };
    let program_a =
        "begin\nlet x = count t where @v == 10\nupdate t where @id == 2 set @v = @v + 1\ncommit\n";
    let program_b =
        "begin\nlet y = count t where @v == 30\nupdate t where @id == 4 set @v = @v + 1\ncommit\n";
    let model = build(
        "serializable",
        json!([{ "name": "t", "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}], "primaryKey": ["id"] }]),
        json!({ "t": [{"id":1,"v":10},{"id":2,"v":20},{"id":3,"v":30},{"id":4,"v":40}] }),
        vec![
            actor("A", program_a, json!({})),
            actor("B", program_b, json!({})),
        ],
        json!([{ "name": "ok", "assert": { "ge": [ { "count": { "table": "t", "where": {} } }, 0 ] } }]),
        json!({}),
        json!({}),
    );
    let (all, _) = enumerate_schedules(&model, 1000);
    let db = env.database(&model).unwrap();
    let mut stricter = 0;
    for s in &all {
        let run = run_schedule(&env, &db, &model, s).unwrap();
        for o in run.observations.iter().filter(|o| !o.agree) {
            assert!(
                o.observed == "error 40001" && !o.predicted.starts_with("error"),
                "a disagreement in the wrong direction: {o:?}"
            );
            stricter += 1;
        }
    }
    eprintln!(
        "{} schedules: PostgreSQL aborted {stricter} time(s) where the model's logical graph had no cycle",
        all.len()
    );
    // Reads through an unindexed column scan the relation: PostgreSQL does
    // abort here (observed on 17.11), the model does not.
    assert!(
        stricter > 0,
        "expected PostgreSQL's relation-level predicate locks to abort some schedule"
    );
}

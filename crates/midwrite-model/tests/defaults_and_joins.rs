//! Column defaults and sequences (allocation is
//! not transactional), `INSERT ... RETURNING`, bounded `INNER`/`LEFT JOIN`
//! with projections, their footprints (reduction must preserve every
//! reachable state), and a PostgreSQL differential over every schedule.

mod common;

use std::collections::BTreeSet;

use common::*;
use midwrite_model::engine::initial_state;
use midwrite_model::explore::{ExploreOptions, explore_with};
use midwrite_model::ir::{Op, ProjItem, StmtKind};
use midwrite_model::model::Model;
use midwrite_model::oracle::{PgEnv, run_schedule};
use midwrite_model::state::State;
use midwrite_model::value::{Row, Value};
use midwrite_model::witness::{enumerate_schedules, replay};
use serde_json::json;

fn committed(model: &Model, s: &State, table: &str) -> Vec<Row> {
    let t = model.schema.index(table).expect("table");
    let mut rows: Vec<Row> = s
        .store
        .table_rows(t)
        .filter_map(|id| {
            s.store
                .latest_committed(id)
                .and_then(|(_, v)| v.values.clone())
        })
        .collect();
    rows.sort();
    rows
}

fn final_states(model: &Model) -> Vec<State> {
    let (all, truncated) = enumerate_schedules(model, 50_000);
    assert!(!truncated, "enumeration truncated");
    all.iter()
        .map(|schedule| replay(model, schedule, false).expect("replays").state)
        .collect()
}

fn invitation_tables() -> serde_json::Value {
    json!([
        { "name": "users", "columns": [
            {"name":"id","notNull":true,"type":"int"},
            {"name":"email","notNull":true,"type":"text"}
          ], "primaryKey": ["id"] },
        { "name": "team_members", "columns": [
            {"name":"id","notNull":true,"type":"int","default":{"sequence":{}}},
            {"name":"userId","notNull":true,"type":"int"},
            {"name":"teamId","notNull":true,"type":"int"}
          ], "primaryKey": ["id"] },
        { "name": "invitations", "columns": [
            {"name":"id","notNull":true,"type":"int","default":{"sequence":{}}},
            {"name":"teamId","notNull":true,"type":"int"},
            {"name":"email","notNull":true,"type":"text"},
            {"name":"status","notNull":true,"type":"text","default":{"value":"pending"}}
          ], "primaryKey": ["id"] }
    ])
}

/// Scenarios must declare an invariant; these tests inspect states directly.
fn trivial_invariant() -> serde_json::Value {
    json!([{ "name": "trivial", "assert": { "le": [0, 1] } }])
}

fn invitation_initial() -> serde_json::Value {
    json!({
        "users": [{"id": 1, "email": "owner@x"}, {"id": 2, "email": "new@x"}],
        "team_members": [{"id": 1, "userId": 1, "teamId": 1}],
        "invitations": []
    })
}

const INVITE_IF_NOT_MEMBER: &str = "\
let m = select users left join team_members on @users.id == @team_members.userId where @users.email == e && @team_members.teamId == t
if m != null {
  fail \"already a member\"
}
insert invitations set @teamId = t, @email = e returning inv
";

const ADD_MEMBER: &str = "\
insert team_members set @userId = u, @teamId = t returning tm
";

const INSERT_THEN_FAIL_IN_TX: &str = "\
begin
insert invitations set @teamId = 1, @email = e returning inv
fail \"boom\"
commit
";

const INSERT_RETURNING: &str = "\
insert invitations set @teamId = 1, @email = e returning inv
";

fn join_model(iso: &str) -> Model {
    build(
        iso,
        invitation_tables(),
        invitation_initial(),
        vec![
            actor(
                "Inviter",
                INVITE_IF_NOT_MEMBER,
                json!({"e": "new@x", "t": 1}),
            ),
            actor("Joiner", ADD_MEMBER, json!({"u": 2, "t": 1})),
        ],
        trivial_invariant(),
        json!({}),
        json!({}),
    )
}

fn sequence_model(iso: &str) -> Model {
    build(
        iso,
        invitation_tables(),
        invitation_initial(),
        vec![
            actor("Failing", INSERT_THEN_FAIL_IN_TX, json!({"e": "a@x"})),
            actor("Plain", INSERT_RETURNING, json!({"e": "b@x"})),
        ],
        trivial_invariant(),
        json!({}),
        json!({}),
    )
}

#[test]
fn sequence_values_are_not_given_back_by_a_rollback() {
    let model = sequence_model("read-committed");
    let mut plain_ids = BTreeSet::new();
    for s in final_states(&model) {
        let rows = committed(&model, &s, "invitations");
        // The failing actor's row never commits.
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].get("email"), Some(&Value::str("b@x")));
        // The default is applied.
        assert_eq!(rows[0].get("status"), Some(&Value::str("pending")));
        let returned = s.actors[1].locals.get("inv").cloned();
        assert_eq!(returned, Some(Value::Row(rows[0].clone())));
        if let Some(Value::Int(id)) = rows[0].get("id") {
            plain_ids.insert(*id);
        }
    }
    // Depending on the order, the rolled-back insert consumed 1 first.
    assert_eq!(plain_ids, BTreeSet::from([1, 2]));
}

#[test]
fn a_sequence_start_follows_the_initial_rows_unless_declared() {
    let model = join_model("read-committed");
    let seqs = model.sequences();
    assert_eq!(
        seqs.iter()
            .map(|(_, c, n)| (c.as_str(), *n))
            .collect::<Vec<_>>(),
        vec![("id", 2), ("id", 1)]
    );
    let s = initial_state(&model);
    assert_eq!(s.seqs, vec![2, 1]);
}

#[test]
fn left_join_sees_or_misses_a_concurrent_membership() {
    let model = join_model("read-committed");
    let mut outcomes = BTreeSet::new();
    for s in final_states(&model) {
        let invited = committed(&model, &s, "invitations").len();
        let members = committed(&model, &s, "team_members").len();
        assert_eq!(members, 2);
        outcomes.insert((invited, s.actors[0].outcome.label()));
    }
    // The membership check passes when it runs before the join commits and
    // fails after it: both are reachable.
    assert_eq!(
        outcomes,
        BTreeSet::from([
            (0, "failed: already a member".to_owned()),
            (1, "ok".to_owned()),
        ])
    );
}

#[test]
fn left_join_result_rows_are_nested_by_table_name() {
    let model = build(
        "read-committed",
        invitation_tables(),
        invitation_initial(),
        vec![actor(
            "Reader",
            "let m = select users left join team_members on @users.id == @team_members.userId where @users.id == 2\n",
            json!({}),
        )],
        trivial_invariant(),
        json!({}),
        json!({}),
    );
    let states = final_states(&model);
    let m = states[0].actors[0].locals.get("m").cloned().expect("bound");
    let Value::Row(row) = m else {
        panic!("not a row: {m:?}")
    };
    assert_eq!(row.get("team_members"), Some(&Value::Null));
    assert!(
        matches!(row.get("users"), Some(Value::Row(u)) if u.get("email") == Some(&Value::str("new@x")))
    );
}

/// The join with a projection, as the extractor produces for
/// `db.select({ user: users, teamId: teamMembers.teamId }).from(users)...`.
fn projected_join(iso: &str) -> Model {
    let mut model = join_model(iso);
    for code in &mut model.actors[0].codes {
        for step in code {
            if let Op::Db { stmt } = &mut step.op
                && let StmtKind::Select { projection, .. } = &mut stmt.kind
            {
                *projection = Some(vec![
                    ("user".to_owned(), ProjItem::Table("users".to_owned())),
                    (
                        "teamId".to_owned(),
                        ProjItem::Column("team_members.teamId".to_owned()),
                    ),
                ]);
            }
        }
    }
    model
}

#[test]
fn reduction_preserves_reachable_states_with_joins_and_sequences() {
    for (label, model) in [
        ("join", join_model("read-committed")),
        ("join-rr", join_model("repeatable-read")),
        ("sequence", sequence_model("read-committed")),
        ("unique-generated", unique_invitations("read-committed")),
        ("unique-generated-ser", unique_invitations("serializable")),
        ("inner-join-rr", inner_join_vs_delete("repeatable-read")),
    ] {
        let mut full = ExploreOptions::from_model(&model);
        full.por = false;
        full.collect_states = true;
        full.stop_at_first = false;
        let mut reduced = full.clone();
        reduced.por = true;
        let a = explore_with(&model, &full).reached.expect("collected");
        let b = explore_with(&model, &reduced).reached.expect("collected");
        assert_eq!(a, b, "{label}: reduction lost states");
    }
}

/// Two concurrent invitations under UNIQUE(teamId, email) with generated
/// ids: one inserts (RETURNING the row), the other does nothing (RETURNING
/// no row) or waits for the first; a third rolls back after RETURNING.
fn unique_invitations(iso: &str) -> Model {
    let mut tables = invitation_tables();
    tables[2]["unique"] = json!([["teamId", "email"]]);
    build(
        iso,
        tables,
        invitation_initial(),
        vec![
            actor(
                "A",
                "insert invitations set @teamId = 1, @email = e on conflict do nothing into n returning inv\n",
                json!({"e": "x@x"}),
            ),
            actor(
                "B",
                "insert invitations set @teamId = 1, @email = e on conflict do nothing into n returning inv\n",
                json!({"e": "x@x"}),
            ),
            actor(
                "C",
                "begin\ninsert invitations set @teamId = 2, @email = e returning inv\nrollback\n",
                json!({"e": "y@x"}),
            ),
        ],
        trivial_invariant(),
        json!({}),
        json!({}),
    )
}

/// An inner join read races a delete of the joined row.
fn inner_join_vs_delete(iso: &str) -> Model {
    build(
        iso,
        invitation_tables(),
        invitation_initial(),
        vec![
            actor(
                "Reader",
                "begin\nlet m = select users inner join team_members on @users.id == @team_members.userId where @users.id == 1\nlet again = select users join team_members on @users.id == @team_members.userId where @users.id == 1\ncommit\n",
                json!({}),
            ),
            actor(
                "Remover",
                "delete team_members where @userId == 1\n",
                json!({}),
            ),
        ],
        trivial_invariant(),
        json!({}),
        json!({}),
    )
}

fn env_or_skip() -> Option<PgEnv> {
    match PgEnv::start() {
        Ok(env) => Some(env),
        Err(reason) => {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "PostgreSQL is required but unavailable: {reason}"
            );
            eprintln!("skipping: {reason}");
            None
        }
    }
}

#[test]
fn joins_returning_and_sequences_agree_with_postgres_on_every_schedule() {
    let Some(mut env) = env_or_skip() else { return };
    let mut configs = Vec::new();
    for iso in ["read-committed", "repeatable-read", "serializable"] {
        configs.push((format!("left-join-membership/{iso}"), join_model(iso)));
        configs.push((format!("projected-join/{iso}"), projected_join(iso)));
        configs.push((format!("sequence-rollback/{iso}"), sequence_model(iso)));
        configs.push((
            format!("unique-generated-on-conflict/{iso}"),
            unique_invitations(iso),
        ));
        configs.push((
            format!("inner-join-vs-delete/{iso}"),
            inner_join_vs_delete(iso),
        ));
    }
    let mut bad = Vec::new();
    let mut runs = 0;
    for (label, model) in &configs {
        let (all, truncated) = enumerate_schedules(model, 20_000);
        assert!(!truncated);
        let db = env
            .database(model)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        for schedule in &all {
            runs += 1;
            match run_schedule(&env, &db, model, schedule) {
                Ok(run) if run.agree => {}
                Ok(run) => bad.push(format!(
                    "{label}: {:?}\n   {}",
                    schedule,
                    run.observations
                        .iter()
                        .filter(|o| !o.agree)
                        .map(|o| format!(
                            "{} predicted {} observed {}",
                            o.sql, o.predicted, o.observed
                        ))
                        .collect::<Vec<_>>()
                        .join("\n   ")
                        + &if run.tables_agree {
                            String::new()
                        } else {
                            format!(
                                "\n   tables: model {:?} postgres {:?}",
                                run.model_tables, run.postgres_tables
                            )
                        }
                )),
                Err(e) => bad.push(format!("{label}: infrastructure: {e}")),
            }
        }
    }
    eprintln!("PostgreSQL {}: {runs} schedule(s)", env.version());
    assert!(
        bad.is_empty(),
        "{} disagreement(s):\n{}",
        bad.len(),
        bad.join("\n")
    );
}

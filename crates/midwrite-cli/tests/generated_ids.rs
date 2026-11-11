//! Generated identifiers, multi-module schemas, repositories.
//!
//! * The Drizzle/PostgreSQL differential runs first-party probe functions
//!   (`fixtures/apps/defaults-probe`) twice: through pinned Drizzle on a
//!   disposable PostgreSQL, and extracted and executed by the model. Final
//!   rows are compared with generated values matched one-to-one.
//! * Discovery on `fixtures/apps/workspace-app` finds the generated-key
//!   hazards on its own and the generated counterexamples reproduce on the
//!   original code.
//!
//! Runs that need PostgreSQL or Node skip without them unless
//! `MIDWRITE_REQUIRE_REPLAY=1`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as Json, json};

use midwrite_analyzer::TsActorProvider;
use midwrite_model::engine::{Recorder, apply, enabled, initial_state};
use midwrite_model::model::Model;
use midwrite_model::value::{GenId, Value};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_midwrite"))
}

macro_rules! require_runtime {
    () => {
        let missing = midwrite_replay::postgres::Binaries::discover()
            .err()
            .or_else(|| {
                Command::new("node")
                    .arg("--version")
                    .output()
                    .is_err()
                    .then(|| "node is not installed".to_owned())
            });
        if let Some(why) = missing {
            assert!(
                std::env::var_os("MIDWRITE_REQUIRE_REPLAY").is_none(),
                "PostgreSQL and Node are required: {why}"
            );
            eprintln!("skipping: {why}");
            return;
        }
    };
}

fn build_fixture(dir: &Path) {
    if dir.join("dist/verify").is_dir() {
        return;
    }
    let status = Command::new("npm")
        .args(["run", "build"])
        .current_dir(dir)
        .status()
        .expect("npm");
    assert!(status.success(), "building {} failed", dir.display());
}

// ---------------------------------------------------------------------------
// Drizzle / PostgreSQL differential
// ---------------------------------------------------------------------------

const PROBE_TABLES: [&str; 5] = ["people", "profiles", "notes", "counters", "tickets"];

/// The model's final rows for one probe function: extracted from the
/// TypeScript source, executed alone by the engine.
fn model_rows(
    probe: &str,
    args: &[&str],
) -> (Model, BTreeMap<String, Vec<BTreeMap<String, Value>>>) {
    let root = repo().join("fixtures/apps/defaults-probe");
    let doc = json!({
        "schemaVersion": 1,
        "name": format!("probe-{probe}"),
        "extraction": {
            "root": ".",
            "schema": ["src/db/schema/people.ts", "src/db/schema/notes.ts"],
        },
        "database": {
            "tables": PROBE_TABLES.iter().map(|t| json!({ "from": t })).collect::<Vec<_>>(),
        },
        "actors": [{
            "name": "probe",
            "entry": { "selector": format!("src/probes.ts#{probe}") },
            "arguments": args,
        }],
        "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }],
    });
    let provider = TsActorProvider::new(&root);
    let (model, _) = midwrite_model::scenario::load_str(&doc.to_string(), &root, &provider)
        .unwrap_or_else(|e| panic!("probe {probe}: {e}"));
    let mut s = initial_state(&model);
    let mut rec = Recorder::off();
    loop {
        let next = enabled(&model, &s)
            .into_iter()
            .find(|t| !t.is_crash_or_retry());
        let Some(t) = next else { break };
        s = apply(&model, &s, t, &mut rec)
            .unwrap_or_else(|u| panic!("probe {probe}: unsupported {}", u.detail))
            .state;
    }
    let mut out = BTreeMap::new();
    for (i, t) in model.schema.tables.iter().enumerate() {
        out.insert(t.name.clone(), s.committed_rows(&model, i as u16));
    }
    (model, out)
}

/// What the probe runner printed for one probe on a fresh database.
fn runtime_probe(socket_dir: &Path, probe: &str, args: &[&str]) -> Json {
    let dir = repo().join("fixtures/apps/defaults-probe");
    let output = Command::new("node")
        .arg("dist/verify/probe.js")
        .arg(probe)
        .args(args)
        .current_dir(&dir)
        .env("PGHOST", socket_dir)
        .env("PGUSER", midwrite_replay::postgres::USER)
        .env("PGDATABASE", "probe")
        .output()
        .expect("node");
    assert!(
        output.status.success(),
        "probe {probe}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("probe JSON")
}

/// Whether the runtime rows equal the model rows, generated values matched
/// one-to-one (the same generated value is always the same text, two
/// generated values never one text). Only modeled columns are compared.
fn same_rows(
    model: &Model,
    expected: &BTreeMap<String, Vec<BTreeMap<String, Value>>>,
    runtime: &Json,
) -> Result<(), String> {
    let mut forward: BTreeMap<GenId, String> = BTreeMap::new();
    let mut backward: BTreeMap<String, GenId> = BTreeMap::new();
    for t in &model.schema.tables {
        let rows = &expected[&t.name];
        let observed = runtime["tables"][&t.name]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if rows.len() != observed.len() {
            return Err(format!(
                "`{}`: model {} row(s), PostgreSQL {}",
                t.name,
                rows.len(),
                observed.len()
            ));
        }
        // Match each model row with an unused runtime row.
        let mut used = vec![false; observed.len()];
        for row in rows {
            let mut found = false;
            for (j, o) in observed.iter().enumerate() {
                if used[j] {
                    continue;
                }
                let (mut f, mut b) = (forward.clone(), backward.clone());
                let ok = t.columns.iter().all(|c| {
                    let sql = c.sql_name.clone().unwrap_or_else(|| c.name.clone());
                    let got = &o[&sql];
                    match row.get(&c.name).unwrap_or(&Value::Null) {
                        Value::Gen(g) => {
                            let Some(text) = got.as_str() else {
                                return false;
                            };
                            match (f.get(g), b.get(text)) {
                                (Some(x), _) => x == text,
                                (None, Some(_)) => false,
                                (None, None) => {
                                    f.insert(*g, text.to_owned());
                                    b.insert(text.to_owned(), *g);
                                    true
                                }
                            }
                        }
                        Value::Int(n) => got.as_i64() == Some(*n),
                        Value::Str(s) => got.as_str() == Some(s.as_str()),
                        Value::Bool(v) => got.as_bool() == Some(*v),
                        Value::Null => got.is_null(),
                        _ => false,
                    }
                });
                if ok {
                    used[j] = true;
                    forward = f;
                    backward = b;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(format!(
                    "`{}`: no PostgreSQL row matches the model row {}",
                    t.name,
                    Value::Row(row.clone()).render()
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn drizzle_generated_defaults_match_the_model() {
    require_runtime!();
    build_fixture(&repo().join("fixtures/apps/defaults-probe"));
    let bins = midwrite_replay::postgres::Binaries::discover().expect("binaries");
    let root = midwrite_replay::safety::ReplayRoot::create(false).expect("private directory");
    let cluster =
        midwrite_replay::postgres::Cluster::start(bins, &root.path.join("db")).expect("cluster");
    cluster.create_database("probe").expect("database");

    // Operations the model represents: rows must agree.
    let supported: [(&str, &[&str]); 6] = [
        ("twoNotes", &[]),
        ("personWithProfile", &["a@example.test"]),
        ("explicitNote", &["fixed-id"]),
        ("duplicatePerson", &["a@example.test"]),
        ("rollbackThenInsert", &[]),
        ("identityAndConstant", &[]),
    ];
    for (probe, args) in supported {
        let (model, expected) = model_rows(probe, args);
        let runtime = runtime_probe(cluster.socket_dir(), probe, args);
        assert!(runtime["error"].is_null(), "{probe}: {}", runtime["error"]);
        if let Err(why) = same_rows(&model, &expected, &runtime) {
            panic!("{probe}: the model and PostgreSQL disagree: {why}\nruntime: {runtime:#}");
        }
    }

    // What Drizzle does with an application default (pinned drizzle-orm),
    // the facts the model's `Generated` semantics rest on.
    let sent = |r: &Json| r["sent"].as_array().cloned().unwrap_or_default();
    // Once per row, as a bound parameter, at every execution of a builder.
    let r = runtime_probe(cluster.socket_dir(), "multiRow", &[]);
    assert_eq!(r["calls"]["stamp"], 2, "{r:#}");
    assert_eq!(r["tables"]["notes"].as_array().unwrap().len(), 2);
    let r = runtime_probe(cluster.socket_dir(), "sameBuilderTwice", &[]);
    let notes = r["tables"]["notes"].as_array().unwrap();
    assert_eq!(
        notes.len(),
        2,
        "a re-executed builder generates a new key: {r:#}"
    );
    assert_ne!(notes[0]["id"], notes[1]["id"]);
    // Evaluated even when the INSERT then fails (its value is discarded).
    let r = runtime_probe(cluster.socket_dir(), "failingInsert", &[]);
    assert_eq!(r["calls"]["stamp"], 1, "{r:#}");
    assert!(r["tables"]["notes"].as_array().unwrap().is_empty());
    // A database default is sent as DEFAULT; RETURNING exposes the value.
    let r = runtime_probe(
        cluster.socket_dir(),
        "personWithProfile",
        &["b@example.test"],
    );
    let first = sent(&r)[0]["sql"].as_str().unwrap().to_owned();
    assert!(
        first.contains("values (default, $1) returning \"id\""),
        "{first}"
    );
    assert_eq!(
        r["tables"]["profiles"][0]["person_id"], r["tables"]["people"][0]["id"],
        "the foreign key carries the returned key"
    );
    // A sequence is not rolled back; an application key is not reused.
    let r = runtime_probe(cluster.socket_dir(), "rollbackThenInsert", &[]);
    assert_eq!(r["tables"]["counters"][0]["id"], 2, "{r:#}");
}

#[test]
fn the_model_does_not_represent_shapes_it_has_not_validated() {
    // A builder executed twice and SQL DEFAULT are observed at runtime
    // (above) but not modeled: extraction says so. (Phase 14 models
    // multi-row inserts: `docs/architecture/postgres-model.md`.)
    let root = repo().join("fixtures/apps/defaults-probe");
    for (probe, args) in [
        ("sameBuilderTwice", json!([])),
        ("sqlDefault", json!(["s@example.test"])),
    ] {
        let doc = json!({
            "schemaVersion": 1,
            "name": probe,
            "extraction": {
                "root": ".",
                "schema": ["src/db/schema/people.ts", "src/db/schema/notes.ts"],
            },
            "database": {
                "tables": PROBE_TABLES.iter().map(|t| json!({ "from": t })).collect::<Vec<_>>(),
            },
            "actors": [{
                "name": "probe",
                "entry": { "selector": format!("src/probes.ts#{probe}") },
                "arguments": args,
            }],
            "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }],
        });
        let provider = TsActorProvider::new(&root);
        let (model, _) =
            midwrite_model::scenario::load_str(&doc.to_string(), &root, &provider).unwrap();
        let unknown = model.actors[0]
            .codes
            .iter()
            .flatten()
            .any(|s| matches!(s.op, midwrite_model::ir::Op::Unknown { .. }));
        assert!(unknown, "{probe} must be reported as unsupported");
    }
}

// ---------------------------------------------------------------------------
// The workspace application
// ---------------------------------------------------------------------------

fn workspace_app() -> PathBuf {
    repo().join("fixtures/apps/workspace-app")
}

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("midwrite-p12-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn discover(extra: &[&str]) -> Json {
    let output = bin()
        .arg("discover")
        .arg(workspace_app())
        .args(["--format", "json", "--no-timing", "--model-check"])
        .args(extra)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn candidate<'a>(report: &'a Json, class: &str, entries: &[&str]) -> &'a Json {
    report["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["class"] == class
                && c["entries"].as_array().unwrap().len() == entries.len()
                && entries.iter().all(|e| {
                    c["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|x| x["selector"].as_str().unwrap().ends_with(e))
                })
        })
        .unwrap_or_else(|| panic!("no {class} candidate for {entries:?}"))
}

#[test]
fn discovery_models_generated_keys_across_schema_modules_and_repositories() {
    let r = discover(&[]);
    let index = &r["index"];
    assert_eq!(index["tablesModeled"], 4, "{index:#}");
    assert!(index["tablesRefused"].as_array().unwrap().is_empty());
    // The race through object and class repositories, with generated keys,
    // is found from the declared rule; the locked version is not.
    let race = candidate(&r, "check-then-act", &["#createWorkspace"]);
    assert_eq!(race["status"], "MODEL_COUNTEREXAMPLE", "{race:#}");
    let control = &race["scenarios"][1];
    assert_eq!(control["variant"]["name"], "distinct-key");
    assert_eq!(
        control["model"]["verdict"], "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        "a control with distinct owners must not fail"
    );
    let locked = candidate(&r, "check-then-act", &["#createWorkspaceLocked"]);
    assert_eq!(locked["status"], "NO_VIOLATION_WITHIN_BOUNDS");
    let invites = candidate(&r, "check-then-act", &["#inviteToWorkspace"]);
    assert_eq!(invites["status"], "NO_VIOLATION_WITHIN_BOUNDS");
    // A resubmitted insert: a regenerated key duplicates, a request key
    // collides.
    let regenerated = candidate(&r, "resubmitted-write", &["#addWorkspace"]);
    assert_eq!(
        regenerated["status"], "MODEL_COUNTEREXAMPLE",
        "{regenerated:#}"
    );
    let stable = candidate(&r, "resubmitted-write", &["#addWorkspaceIdempotent"]);
    assert_eq!(stable["status"], "NO_VIOLATION_WITHIN_BOUNDS", "{stable:#}");
    assert!(stable["scenarios"][0]["model"]["states"].as_u64().unwrap() > 2);
}

#[test]
fn generated_id_counterexamples_reproduce_on_the_original_code() {
    require_runtime!();
    build_fixture(&workspace_app());
    let out = Temp::new("emit");
    let r = discover(&["--emit-scenarios", out.0.to_str().unwrap()]);
    for (class, entry, level) in [
        (
            "check-then-act",
            "#createWorkspace",
            "original-code-reproduced",
        ),
        (
            "resubmitted-write",
            "#addWorkspace",
            "original-code-crash-reproduced",
        ),
    ] {
        let c = candidate(&r, class, &[entry]);
        let file = c["scenarios"][0]["file"].as_str().unwrap();
        let output = bin()
            .arg("model-check")
            .arg(out.0.join(file))
            .args([
                "--verify-source",
                "--allow-execution",
                "--source-repeat",
                "3",
                "--format",
                "json",
                "--no-timing",
            ])
            .output()
            .unwrap();
        let mc: Json = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(mc["evidence"]["level"], level, "{entry}: {mc:#}");
        // The published report schema (v3) admits only these schema-fact
        // fields: multi-module scenarios must not add any.
        for fact in mc["schemaFacts"].as_array().unwrap() {
            for key in fact.as_object().unwrap().keys() {
                assert!(
                    ["table", "binding", "facts"].contains(&key.as_str()),
                    "{key}"
                );
            }
        }
        let runs = mc["sourceVerification"]["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 3);
        assert!(runs.iter().all(|x| x["reproduced"] == true), "{entry}");
        if entry == "#addWorkspace" {
            // The retry inserted a second workspace with a NEW key: the
            // membership points at the second one.
            let ws = mc["sourceVerification"]["runtimeTables"]["workspaces"]
                .as_array()
                .unwrap();
            assert_eq!(ws.len(), 2);
            assert_ne!(ws[0]["id"], ws[1]["id"]);
        }
    }
}

#[test]
fn a_foreign_key_lock_is_modeled_and_the_schedule_reproduces_at_runtime() {
    // Phase 12 pinned a model limitation here (POSTGRESQL-DIFFERENTIAL.md
    // §5): the unlocked actor's INSERT references the owner's row that
    // `createWorkspaceLocked` holds FOR UPDATE; PostgreSQL's foreign-key
    // check waits for that lock, the Phase 12 model let it proceed, and the
    // original-code run diverged. Phase 13 models foreign keys and their
    // row locks (docs/architecture/postgres-model.md): discovery enables
    // them, the model's counterexample makes the INSERT wait, and the
    // original code follows that schedule and violates the invariant.
    require_runtime!();
    build_fixture(&workspace_app());
    let out = Temp::new("fk-lock");
    let r = discover(&["--emit-scenarios", out.0.to_str().unwrap()]);
    let c = candidate(
        &r,
        "check-then-act",
        &["#createWorkspace", "#createWorkspaceLocked"],
    );
    assert!(
        !c["assumptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "foreign-key-locks"),
        "{c:#}"
    );
    let file = c["scenarios"][0]["file"].as_str().unwrap();
    let scenario: Json =
        serde_json::from_str(&std::fs::read_to_string(out.0.join(file)).unwrap()).unwrap();
    // Discovery extracts with JavaScript value semantics; the
    // inlined helpers' result slots are `undefined`, so the scenario also
    // carries `js-values` (the loader then extracts what discovery did).
    assert_eq!(scenario["features"], json!(["foreign-keys", "js-values"]));
    let output = bin()
        .arg("model-check")
        .arg(out.0.join(file))
        .args([
            "--verify-source",
            "--allow-execution",
            "--format",
            "json",
            "--no-timing",
        ])
        .output()
        .unwrap();
    let mc: Json = serde_json::from_slice(&output.stdout).unwrap();
    let blocked = mc["counterexample"]["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["events"].as_array().into_iter().flatten())
        .any(|e| e["kind"] == "blocked");
    assert!(blocked, "the INSERT waits for the FOR UPDATE lock: {mc:#}");
    let sv = &mc["sourceVerification"];
    assert_eq!(sv["status"], "reproduced", "{sv:#}");
    assert_eq!(mc["evidence"]["level"], "original-code-reproduced");
}

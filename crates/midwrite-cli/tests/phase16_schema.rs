//! Phase 16: the published JSON Schemas of the discovery report
//! (`docs/phase16/schemas/discovery-report.v1.schema.json`) and of the
//! counterexample reproduction artifact
//! (`docs/phase16/schemas/counterexample.v1.schema.json`).
//!
//! Positive fixtures are real reports: every committed discovery report
//! (Phase 11–13 producers) and reports generated here by running
//! `midwrite discover` on first-party fixtures, with and without
//! `--model-check`. Negative fixtures are single mutations of real reports
//! (`fixtures/phase16/schema/negative-cases.json`) that each schema must
//! reject.
//!
//! Validation runs `scripts/validate-schemas.py`: always with its strict
//! built-in validator (`--builtin`, no Python packages needed), and also
//! with the `jsonschema` package when it is importable. Without `python3`
//! the tests skip with a message, unless `MIDWRITE_REQUIRE_SCHEMA=1`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde_json::Value as Json;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const DISCOVERY_SCHEMA: &str = "docs/phase16/schemas/discovery-report.v1.schema.json";
const COUNTEREXAMPLE_SCHEMA: &str = "docs/phase16/schemas/counterexample.v1.schema.json";

/// The validator modes available here: `--builtin` always, the
/// `jsonschema` package when importable. `None`: no `python3` (skip).
fn modes() -> Option<Vec<&'static str>> {
    static MODES: OnceLock<Option<Vec<&'static str>>> = OnceLock::new();
    MODES
        .get_or_init(|| {
            let python = Command::new("python3").arg("--version").output();
            if !python.is_ok_and(|o| o.status.success()) {
                assert!(
                    std::env::var_os("MIDWRITE_REQUIRE_SCHEMA").is_none(),
                    "MIDWRITE_REQUIRE_SCHEMA is set but python3 is not available"
                );
                eprintln!("skipping: python3 is not available to run scripts/validate-schemas.py");
                return None;
            }
            let jsonschema = Command::new("python3")
                .args(["-c", "import jsonschema, referencing"])
                .output()
                .is_ok_and(|o| o.status.success());
            if jsonschema {
                Some(vec!["--builtin", "jsonschema"])
            } else {
                eprintln!(
                    "note: the Python `jsonschema` package is not installed; validating with the built-in strict validator only"
                );
                Some(vec!["--builtin"])
            }
        })
        .clone()
}

macro_rules! require_python {
    () => {
        match modes() {
            Some(m) => m,
            None => return,
        }
    };
}

struct Outcome {
    success: bool,
    stdout: String,
    stderr: String,
}

/// `scripts/validate-schemas.py [--builtin] [--schema S] FILES...`.
fn validate(mode: &str, schema: Option<&str>, files: &[PathBuf]) -> Outcome {
    let mut cmd = Command::new("python3");
    cmd.arg(repo().join("scripts/validate-schemas.py"));
    if mode == "--builtin" {
        cmd.arg("--builtin");
    }
    if let Some(s) = schema {
        cmd.arg("--schema").arg(repo().join(s));
    }
    cmd.args(files);
    let out = cmd.output().expect("python3");
    Outcome {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Per validated file, in order: its name, its number of schema errors and
/// whether the corrupted copy was rejected (the script's summary lines).
fn summary_lines(stdout: &str) -> Vec<(String, (usize, bool))> {
    let mut out = Vec::new();
    for line in stdout.lines().filter(|l| !l.starts_with(' ')) {
        let Some((head, tail)) = line.split_once(" schema errors; corrupted copy rejected: ")
        else {
            continue;
        };
        let errors = head
            .rsplit(' ')
            .next()
            .and_then(|n| n.parse().ok())
            .expect("error count");
        let name = head.split(": ").nth(1).expect("file name").to_owned();
        out.push((name, (errors, tail == "True")));
    }
    out
}

fn assert_valid(files: &[PathBuf], what: &str) {
    let modes = require_python!();
    for mode in modes {
        let o = validate(mode, None, files);
        assert!(
            o.success,
            "{what} ({mode}) failed schema validation:\n{}{}",
            o.stdout, o.stderr
        );
        let lines = summary_lines(&o.stdout);
        assert_eq!(lines.len(), files.len(), "{what} ({mode}):\n{}", o.stdout);
        for (name, (errors, rejected)) in &lines {
            assert_eq!(*errors, 0, "{what} ({mode}) {name}:\n{}", o.stdout);
            assert!(rejected, "{what} ({mode}) {name}: corrupted copy accepted");
        }
    }
}

fn read(path: &Path) -> Json {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json")
}

/// The committed reports of `kind` `commitrift.discovery`.
fn committed_discovery_reports() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![repo().join("reports")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("reports") {
            let path = e.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "json")
                && std::fs::read_to_string(&path)
                    .is_ok_and(|t| t.contains("\"kind\": \"commitrift.discovery\""))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn committed_discovery_reports_validate() {
    let files = committed_discovery_reports();
    // Phase 11 (3), Phase 12 (3, macOS and two Linux runs), Phase 13 (1).
    assert!(files.len() >= 7, "{files:?}");
    for f in &files {
        assert_eq!(read(f)["schemaVersion"], 1, "{}", f.display());
    }
    assert_valid(&files, "committed discovery reports");
}

/// The command the Phase 13–15 evidence and Linux scripts run
/// (`scripts/phase13-linux-docker.sh`): it failed while no discovery
/// schema was published.
#[test]
fn phase13_evidence_schema_check_passes() {
    let _ = require_python!();
    let dir = repo().join("reports/phase13/evidence");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("evidence")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let o = validate("--builtin", None, &files);
    assert!(o.success, "{}{}", o.stdout, o.stderr);
    assert!(
        o.stdout.contains(
            "discovery-report.v1 (builtin validator): workspace-app-discovery.json: 19 candidates, 0 schema errors; corrupted copy rejected: True"
        ),
        "{}",
        o.stdout
    );
}

/// Reports generated from first-party fixtures (once per test binary).
struct Generated {
    reports: Vec<PathBuf>,
    counterexamples: Vec<PathBuf>,
    worlds_report: PathBuf,
}

fn generated() -> &'static Generated {
    static GENERATED: OnceLock<Generated> = OnceLock::new();
    GENERATED.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phase16-schema");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fixtures = repo().join("fixtures");
        let labels = fixtures.join("phase11/labels.json");
        let labels = labels.to_str().unwrap();
        // (fixture, name, extra arguments)
        let runs: &[(&str, &str, &[&str])] = &[
            ("phase15/worlds-app", "worlds-static", &[]),
            ("phase15/worlds-app", "worlds-mc", &["--model-check"]),
            (
                "phase15/worlds-app",
                "worlds-single-world",
                &["--model-check", "--max-worlds", "1"],
            ),
            (
                "phase15/worlds-app",
                "worlds-source-dpor",
                &["--model-check", "--source-dpor"],
            ),
            // Phase 16 vocabulary: loop / caught-error / request
            // assumptions, generated-identity formulas, multi-row locks,
            // NULLS NOT DISTINCT, zod 4 schemas.
            ("phase16/genid", "genid16-mc", &["--model-check"]),
            ("phase16/retry", "retry16-mc", &["--model-check"]),
            ("phase16/locks", "locks16-mc", &["--model-check"]),
            ("phase16/nulls", "nulls16-mc", &["--model-check"]),
            ("phase16/zod/app", "zod16-mc", &["--model-check"]),
            ("phase15/eval-app", "eval15-static", &[]),
            ("phase15/eval-app", "eval15-mc", &["--model-check"]),
            ("phase14/eval-app", "eval14-mc", &["--model-check"]),
            ("phase14/values-app", "values14-mc", &["--model-check"]),
            ("phase13/eval-app", "eval13-mc", &["--model-check"]),
            (
                "phase12/workspace-app",
                "workspace12-mc",
                &["--model-check"],
            ),
            ("phase11/commerce-app", "commerce11-static", &[]),
            (
                "phase11/commerce-app",
                "commerce11-labels",
                &["--model-check", "--labels", labels],
            ),
        ];
        let mut reports = Vec::new();
        let mut counterexamples = Vec::new();
        for (fixture, name, extra) in runs {
            let out = dir.join(format!("{name}.json"));
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_midwrite"));
            cmd.arg("discover")
                .arg(fixtures.join(fixture))
                .args(["--format", "json", "--no-timing", "--output"])
                .arg(&out)
                .args(*extra);
            let emit = dir.join(format!("{name}-scenarios"));
            if extra.contains(&"--model-check") {
                std::fs::create_dir_all(&emit).unwrap();
                cmd.arg("--emit-scenarios").arg(&emit);
            }
            let status = cmd.output().expect("midwrite").status;
            // 0: no model counterexample, 1: model counterexamples found.
            assert!(
                matches!(status.code(), Some(0 | 1)),
                "discover {fixture} {extra:?}: {status}"
            );
            reports.push(out);
            if emit.is_dir() {
                let mut found: Vec<PathBuf> = std::fs::read_dir(&emit)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .filter(|p| {
                        p.file_name()
                            .is_some_and(|n| n.to_string_lossy().ends_with(".counterexample.json"))
                    })
                    .collect();
                found.sort();
                counterexamples.extend(found);
            }
        }
        Generated {
            worlds_report: dir.join("worlds-mc.json"),
            reports,
            counterexamples,
        }
    })
}

#[test]
fn fresh_discovery_reports_of_first_party_fixtures_validate() {
    let _ = require_python!();
    let g = generated();
    let mut statuses = BTreeSet::new();
    let (mut coverage, mut worlds, mut evaluation, mut timings) = (false, false, false, false);
    for f in &g.reports {
        let r = read(f);
        evaluation |= r.get("evaluation").is_some();
        timings |= r.get("timings").is_some();
        for c in r["candidates"].as_array().unwrap() {
            statuses.insert(c["status"].as_str().unwrap().to_owned());
            assert!(c.get("nextStep").is_some(), "{}", f.display());
            coverage |= c.get("coverage").is_some();
            worlds |= c["scenarios"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s.get("world").is_some());
        }
    }
    eprintln!("statuses covered by the generated reports: {statuses:?}");
    for s in [
        "STATIC_CANDIDATE",
        "MODEL_COUNTEREXAMPLE",
        "NO_VIOLATION_WITHIN_BOUNDS",
    ] {
        assert!(statuses.contains(s), "no {s} candidate in {statuses:?}");
    }
    assert!(
        coverage && worlds && evaluation,
        "coverage, worlds, evaluation"
    );
    assert!(!timings, "--no-timing");
    assert_valid(&g.reports, "generated discovery reports");
}

/// The Phase 14 shape of schema version 1 (`nextStep`, no world counters,
/// coverage or worlds) has no committed report; it is derived from a
/// current report without `--model-check` by removing the Phase 15 world
/// counters and scenario `world`s, and the Phase 11–13 shape by also
/// removing `nextStep`.
#[test]
fn earlier_shapes_of_schema_version_1_are_accepted() {
    let _ = require_python!();
    let g = generated();
    let mut phase14 = read(&g.reports[0]);
    assert!(
        phase14["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.get("coverage").is_none() && c.get("nextStep").is_some())
    );
    for k in [
        "maxWorldsPerVariant",
        "worldsBuilt",
        "worldsInvalid",
        "worldsDuplicate",
        "worldsSkippedForBudget",
    ] {
        assert!(remove(&mut phase14, &format!("/budgets/{k}")), "{k}");
    }
    // World search (on by default) annotates scenarios even without
    // `--model-check`.
    for c in phase14["candidates"].as_array_mut().unwrap() {
        for s in c["scenarios"].as_array_mut().unwrap() {
            s.as_object_mut().unwrap().remove("world");
        }
    }
    let mut phase13 = phase14.clone();
    for c in phase13["candidates"].as_array_mut().unwrap() {
        c.as_object_mut().unwrap().remove("nextStep");
    }
    let dir = g.worlds_report.parent().unwrap().join("earlier");
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<PathBuf> = [("phase14-shape", &phase14), ("phase11-13-shape", &phase13)]
        .into_iter()
        .map(|(name, doc)| {
            let file = dir.join(format!("{name}.json"));
            std::fs::write(&file, serde_json::to_string_pretty(doc).unwrap()).unwrap();
            file
        })
        .collect();
    assert_valid(&files, "earlier shapes of schema version 1");
}

#[test]
fn counterexample_artifacts_validate() {
    let _ = require_python!();
    let g = generated();
    assert!(!g.counterexamples.is_empty(), "no counterexample artifact");
    assert_valid(&g.counterexamples, "counterexample artifacts");
}

fn remove(doc: &mut Json, pointer: &str) -> bool {
    let (parent, key) = pointer.rsplit_once('/').expect("pointer");
    match doc.pointer_mut(parent) {
        Some(Json::Object(m)) => m.remove(key).is_some(),
        Some(Json::Array(a)) => match key.parse::<usize>() {
            Ok(i) if i < a.len() => {
                a.remove(i);
                true
            }
            _ => false,
        },
        _ => false,
    }
}

fn set(doc: &mut Json, pointer: &str, value: Json) -> bool {
    let (parent, key) = pointer.rsplit_once('/').expect("pointer");
    match doc.pointer_mut(parent) {
        Some(Json::Object(m)) => {
            m.insert(key.to_owned(), value);
            true
        }
        Some(Json::Array(a)) => match key.parse::<usize>() {
            Ok(i) if i < a.len() => {
                a[i] = value;
                true
            }
            _ => false,
        },
        _ => false,
    }
}

#[test]
fn negative_fixtures_are_rejected() {
    let modes = require_python!();
    let g = generated();
    let cases = read(&repo().join("fixtures/phase16/schema/negative-cases.json"));
    let bases: BTreeMap<&str, Json> = [
        ("phase15", read(&g.worlds_report)),
        (
            "phase13",
            read(&repo().join("reports/phase13/evidence/workspace-app-discovery.json")),
        ),
        (
            "counterexample",
            read(
                g.counterexamples
                    .first()
                    .expect("a counterexample artifact"),
            ),
        ),
    ]
    .into_iter()
    .collect();
    let dir = g.worlds_report.parent().unwrap().join("negative");
    std::fs::create_dir_all(&dir).unwrap();
    let mut by_schema: BTreeMap<&str, Vec<PathBuf>> = BTreeMap::new();
    let mut versions = Vec::new();
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let base = case["base"].as_str().unwrap();
        let mut doc = bases[base].clone();
        let applied = if let Some(p) = case.get("remove") {
            remove(&mut doc, p.as_str().unwrap())
        } else {
            set(
                &mut doc,
                case["set"].as_str().unwrap(),
                case["value"].clone(),
            )
        };
        assert!(
            applied,
            "negative case `{name}`: its path does not exist in the `{base}` report (fixture drift)"
        );
        let file = dir.join(format!("{name}.json"));
        std::fs::write(&file, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        if name.contains("unsupported-schema-version") {
            versions.push(file.clone());
        }
        let schema = if base == "counterexample" {
            COUNTEREXAMPLE_SCHEMA
        } else {
            DISCOVERY_SCHEMA
        };
        by_schema.entry(schema).or_default().push(file);
    }
    let total: usize = by_schema.values().map(Vec::len).sum();
    assert!(total >= 30, "{total} negative cases");
    for mode in &modes {
        for (schema, files) in &by_schema {
            let o = validate(mode, Some(schema), files);
            assert!(!o.success, "{mode} {schema}: accepted\n{}", o.stdout);
            let lines = summary_lines(&o.stdout);
            assert_eq!(
                lines.len(),
                files.len(),
                "{mode}:\n{}{}",
                o.stdout,
                o.stderr
            );
            for (name, (errors, _)) in &lines {
                assert!(*errors > 0, "{mode} {schema}: `{name}` was accepted");
            }
        }
        // Without `--schema` an unsupported version has no schema at all.
        for file in &versions {
            let o = validate(mode, None, std::slice::from_ref(file));
            assert!(!o.success);
            assert!(
                o.stderr.contains("no schema for kind/version"),
                "{}: {}",
                file.display(),
                o.stderr
            );
        }
    }
}

/// Phase 16 text help (rendering only): a BLOCKED candidate gets a
/// declaration skeleton, an INCONCLUSIVE one the contract fields that
/// exist, a MODEL_COUNTEREXAMPLE with an emitted scenario the commands
/// and files that reproduce it; the output is deterministic.
#[test]
fn text_output_names_declarations_and_reproduction_files() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phase16-schema-text");
    let _ = std::fs::remove_dir_all(&dir);
    let emit = dir.join("scenarios");
    std::fs::create_dir_all(&emit).unwrap();
    let run = || {
        let out = Command::new(env!("CARGO_BIN_EXE_midwrite"))
            .arg("discover")
            .arg(repo().join("fixtures/phase12/workspace-app"))
            .args(["--model-check", "--no-timing", "--emit-scenarios"])
            .arg(&emit)
            .output()
            .expect("midwrite");
        assert!(matches!(out.status.code(), Some(0 | 1)), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let text = run();
    assert_eq!(text, run(), "text output is not deterministic");
    for needle in [
        r#"Declare a property: in midwrite-contracts.json, "invariants": [{"name": "<rule-name>", "entries": ["src/services/invites.ts#inviteToWorkspace"], "property": {"template": "atMostOne", "table": "invites", "key": ["<column>"]}}]"#,
        r#"Declare inputs: in midwrite-contracts.json, "entries": {"#,
        "under `entries` (`values` for parameter paths, or positional `arguments`)",
        "Reproduce in the model: midwrite model-check ",
        "Reproduction artifact: ",
    ] {
        assert!(text.contains(needle), "missing `{needle}` in\n{text}");
    }
    assert!(!text.contains("`initialState`"), "{text}");
    // Every file the text names exists.
    let mut named = 0;
    for line in text.lines() {
        let path = line
            .trim()
            .strip_prefix("Reproduce in the model: midwrite model-check ")
            .or_else(|| line.trim().strip_prefix("Reproduction artifact: "));
        if let Some(p) = path {
            assert!(Path::new(p).is_file(), "{p} does not exist");
            named += 1;
        }
    }
    assert!(named >= 2, "{text}");
}

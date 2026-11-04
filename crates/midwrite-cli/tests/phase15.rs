//! Phase 15: input-dependent defects end to end on the first-party worlds
//! application (`fixtures/phase15/worlds-app`): the feedback-guided search
//! finds each defect in a world other than the synthesized one, writes a
//! reproduction artifact, the partial-order reductions agree on every
//! world, and the original code follows each counterexample against a
//! disposable PostgreSQL.
//!
//! Runs that need PostgreSQL or Node skip without them unless
//! `MIDWRITE_REQUIRE_REPLAY=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;

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

fn app() -> PathBuf {
    repo().join("fixtures/phase15/worlds-app")
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

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "midwrite-phase15-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
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
        .arg(app())
        .args(["--model-check", "--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("{e}\n{}", String::from_utf8_lossy(&output.stderr));
    })
}

fn candidate<'a>(report: &'a Json, entry: &str) -> &'a Json {
    report["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["selector"].as_str().unwrap().ends_with(entry))
        })
        .unwrap_or_else(|| panic!("no candidate of {entry}: {report:#}"))
}

/// The emitted scenario of the world that decided the candidate.
fn deciding_file(c: &Json) -> (String, String) {
    let s = c["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["model"]["verdict"] == "COUNTEREXAMPLE_FOUND")
        .unwrap_or_else(|| panic!("a counterexample world: {c:#}"));
    (
        s["file"].as_str().unwrap().to_owned(),
        s["world"]["id"].as_str().unwrap().to_owned(),
    )
}

fn model_check(file: &Path, extra: &[&str]) -> Json {
    let output = bin()
        .arg("model-check")
        .arg(file)
        .args(["--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("{e}\n{}", String::from_utf8_lossy(&output.stderr));
    })
}

#[test]
fn counterexamples_found_in_other_worlds_come_with_a_reproduction_artifact() {
    let out = Temp::new("repro");
    let r = discover(&["--emit-scenarios", out.0.to_str().unwrap()]);
    for (entry, world) in [
        ("#createFolder", "null:parentId"),
        ("#startExport", "stored-boundary:quotas.remaining=1"),
    ] {
        let c = candidate(&r, entry);
        assert_eq!(c["status"], "MODEL_COUNTEREXAMPLE", "{c:#}");
        let (file, id) = deciding_file(c);
        assert_eq!(id, world);
        let repro = out.0.join(file.replace(".json", ".counterexample.json"));
        let doc: Json = serde_json::from_str(&std::fs::read_to_string(&repro).unwrap()).unwrap();
        assert_eq!(doc["kind"], "commitrift.counterexample");
        assert_eq!(doc["world"]["id"], world);
        assert_eq!(doc["status"], "MODEL_COUNTEREXAMPLE");
        assert_eq!(
            doc["evidence"],
            "model counterexample (not an original-code reproduction)"
        );
        assert!(doc["schedule"].as_array().is_some_and(|s| !s.is_empty()));
        assert!(
            doc["sourceFingerprint"]
                .as_array()
                .is_some_and(|s| !s.is_empty())
        );
    }
    // No artifact for a candidate that is not a finding. (Phase 16: the
    // model enforces the NULLS NOT DISTINCT key, so the safe counterpart
    // has no violation within the bounds; Phase 15 reported INCONCLUSIVE.)
    let label = candidate(&r, "#createLabel");
    assert_eq!(label["status"], "NO_VIOLATION_WITHIN_BOUNDS", "{label:#}");
    let names: Vec<String> = std::fs::read_dir(&out.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let label_id = label["id"].as_str().unwrap().to_ascii_lowercase();
    assert!(
        names
            .iter()
            .all(|n| !(n.starts_with(&label_id) && n.ends_with(".counterexample.json"))),
        "{names:?}"
    );
}

#[test]
fn the_reductions_agree_with_the_unreduced_search_on_every_world() {
    let out = Temp::new("por");
    discover(&["--emit-scenarios", out.0.to_str().unwrap()]);
    let mut checked = 0;
    for e in std::fs::read_dir(&out.0).unwrap() {
        let path = e.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".json") || name.ends_with(".counterexample.json") {
            continue;
        }
        let verdicts: Vec<(String, Json)> = [
            ("unreduced", vec!["--no-por"]),
            ("sleep-sets", vec![]),
            ("classic-dpor", vec!["--dpor"]),
            ("source-dpor", vec!["--source-dpor"]),
        ]
        .into_iter()
        .map(|(label, args)| {
            let mc = model_check(&path, &args);
            (
                label.to_owned(),
                serde_json::json!([mc["exploration"]["verdict"], mc["exploration"]["complete"]]),
            )
        })
        .collect();
        for (label, v) in &verdicts[1..] {
            assert_eq!(
                v, &verdicts[0].1,
                "{name}: {label} disagrees with the unreduced search"
            );
        }
        checked += 1;
    }
    assert!(checked >= 8, "every emitted world was checked ({checked})");
}

#[test]
fn input_dependent_counterexamples_reproduce_on_the_original_code() {
    require_runtime!();
    build_fixture(&app());
    let out = Temp::new("source");
    let r = discover(&["--emit-scenarios", out.0.to_str().unwrap()]);
    for entry in ["#createFolder", "#startExport"] {
        let (file, _) = deciding_file(candidate(&r, entry));
        let mc = model_check(
            &out.0.join(&file),
            &[
                "--verify-source",
                "--allow-execution",
                "--source-repeat",
                "2",
            ],
        );
        let sv = &mc["sourceVerification"];
        assert_eq!(sv["status"], "reproduced", "{entry}: {sv:#}");
        assert_eq!(
            mc["evidence"]["level"], "original-code-reproduced",
            "{mc:#}"
        );
    }
    // Phase 16: the NULLS NOT DISTINCT table. Phase 15's model did not
    // enforce the rule and its schedule did not happen on PostgreSQL
    // (not reproduced). The model now enforces it: no counterexample in any
    // world, and its race schedule in the NULL world (both reads, then both
    // inserts) runs on the original code with the same statement outcomes
    // and the invariant holding (a recovery probe without crash or retry).
    let label = candidate(&r, "#createLabel");
    let null_world = label["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["world"]["id"] == "null:parentId")
        .unwrap_or_else(|| panic!("the null world: {label:#}"));
    assert_eq!(
        null_world["model"]["verdict"], "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        "{label:#}"
    );
    let file = out.0.join(null_world["file"].as_str().unwrap());
    let mut doc: Json = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let actors: Vec<String> = doc["actors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap().to_owned())
        .collect();
    doc["sourceVerification"]["recoveryProbe"] = serde_json::json!({
        "require": [
            format!("statement:{}", actors[0]),
            format!("statement:{}", actors[1]),
            format!("statement:{}", actors[0]),
            format!("statement:{}", actors[1]),
        ]
    });
    let probe = out.0.join("label-null-world-probe.json");
    std::fs::write(&probe, doc.to_string()).unwrap();
    let mc = model_check(
        &probe,
        &[
            "--verify-source",
            "--allow-execution",
            "--source-repeat",
            "1",
        ],
    );
    assert_eq!(
        mc["exploration"]["verdict"], "NO_COUNTEREXAMPLE_WITHIN_BOUNDS",
        "{mc:#}"
    );
    assert_eq!(
        mc["sourceVerification"]["status"], "invariant-held",
        "{mc:#}"
    );
    assert_eq!(
        mc["evidence"]["level"], "original-code-invariant-held",
        "{mc:#}"
    );
}

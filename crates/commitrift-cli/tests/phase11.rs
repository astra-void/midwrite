//! Phase 11 end to end: `commitrift discover` on the first-party commerce
//! application, its emitted scenarios loaded by `commitrift model-check`,
//! and selected automatically generated counterexamples run against the
//! ORIGINAL application code on a disposable PostgreSQL.
//!
//! Tests that run application code need Node.js >= 22, the PostgreSQL
//! server binaries (14+) and `npm ci` in fixtures/phase11/commerce-app;
//! without them they print SKIPPED and return, unless
//! `COMMITRIFT_REQUIRE_REPLAY=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn app() -> PathBuf {
    repo().join("fixtures/phase11/commerce-app")
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_commitrift"))
}

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("commitrift-p11-{}-{name}", std::process::id()));
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

fn discover(root: &Path, extra: &[&str]) -> (i32, Value) {
    let output = bin()
        .arg("discover")
        .arg(root)
        .args(["--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap_or(-1), json)
}

fn model_check(path: &Path, extra: &[&str]) -> (i32, Value) {
    let output = bin()
        .arg("model-check")
        .arg(path)
        .args(["--format", "json", "--no-timing"])
        .args(extra)
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap_or(-1), json)
}

fn missing_dependency() -> Option<String> {
    static CHECKED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CHECKED
        .get_or_init(|| {
            match commitrift_replay::postgres::Binaries::discover() {
                Err(_) => {
                    return Some("the PostgreSQL server binaries are not installed".to_owned());
                }
                Ok(b) if commitrift_replay::postgres::major_version(&b.version) < 14 => {
                    return Some(format!("{} is older than 14", b.version));
                }
                Ok(_) => {}
            }
            let Ok(output) = Command::new("node").arg("--version").output() else {
                return Some("Node.js is not installed".to_owned());
            };
            let text = String::from_utf8_lossy(&output.stdout);
            let major: u32 = text
                .trim()
                .trim_start_matches('v')
                .split('.')
                .next()
                .and_then(|m| m.parse().ok())
                .unwrap_or(0);
            if major < 22 {
                return Some(format!("Node.js {} is older than 22", text.trim()));
            }
            if !app()
                .join("node_modules/drizzle-orm/package.json")
                .is_file()
            {
                return Some("run `npm ci` in fixtures/phase11/commerce-app".to_owned());
            }
            let out = Command::new("node")
                .args(["node_modules/typescript/bin/tsc", "-p", "tsconfig.json"])
                .current_dir(app())
                .output()
                .map_err(|e| e.to_string());
            match out {
                Ok(o) if o.status.success() => None,
                Ok(o) => Some(format!(
                    "the commerce application does not compile: {}",
                    String::from_utf8_lossy(&o.stdout)
                )),
                Err(e) => Some(format!("cannot run tsc: {e}")),
            }
        })
        .clone()
}

macro_rules! require_source_run {
    () => {
        if let Some(reason) = missing_dependency() {
            assert!(
                std::env::var_os("COMMITRIFT_REQUIRE_REPLAY").is_none(),
                "source-verification dependencies required: {reason}"
            );
            eprintln!("SKIPPED: {reason}");
            return;
        }
    };
}

#[test]
fn discovery_is_static_and_its_exit_code_reports_counterexamples() {
    let (code, r) = discover(&app(), &[]);
    assert_eq!(code, 0, "no model checking: no counterexample, exit 0");
    assert_eq!(r["kind"], "commitrift.discovery");
    assert_eq!(r["schemaVersion"], 1);
    assert_eq!(r["executesApplicationCode"], false);
    assert_eq!(r["mode"], "discover");
    assert_eq!(r["summary"]["modelCounterexamples"], 0);
    assert!(r["candidates"].as_array().unwrap().iter().all(|c| {
        c["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s.get("model").is_none())
    }));
    let (code, r) = discover(&app(), &["--model-check"]);
    assert_eq!(code, 1, "counterexamples: exit 1");
    assert_eq!(r["mode"], "discover+model-check");
    assert!(r["summary"]["modelCounterexamples"].as_u64().unwrap() >= 9);
    // No absolute path of the machine in the report.
    let text = r.to_string();
    assert!(!text.contains(repo().canonicalize().unwrap().to_string_lossy().as_ref()));
}

#[test]
fn text_output_explains_candidates_and_never_calls_a_hypothesis_a_bug() {
    let output = bin()
        .args(["discover", "--model-check", "--entry", "inviteMember"])
        .arg(app())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    for needle in [
        "Candidate CR-SYNTH-",
        "Entry:",
        "Conflict:",
        "[check] SELECT invitations",
        "[act] INSERT invitations",
        "Resource: invitations(",
        "origin: HEURISTIC_HYPOTHESIS",
        "No supported UNIQUE constraint",
        "Status: MODEL_COUNTEREXAMPLE",
        "this is not a confirmed defect",
        "is not a proof",
    ] {
        assert!(text.contains(needle), "missing `{needle}` in\n{text}");
    }
    assert!(!text.to_lowercase().contains("confirmed bug"));
}

#[test]
fn emitted_scenarios_load_and_give_the_same_verdicts_in_model_check() {
    let out = Temp::new("emit");
    let (_, r) = discover(
        &app(),
        &["--model-check", "--emit-scenarios", out.0.to_str().unwrap()],
    );
    let mut compared = 0;
    for c in r["candidates"].as_array().unwrap() {
        for s in c["scenarios"].as_array().unwrap() {
            let (Some(file), Some(model)) = (s["file"].as_str(), s.get("model")) else {
                continue;
            };
            let (_, mc) = model_check(&out.0.join(file), &[]);
            assert_eq!(mc["exploration"]["verdict"], model["verdict"], "{file}");
            assert_eq!(
                mc["exploration"]["metrics"]["states"], model["states"],
                "{file}"
            );
            compared += 1;
        }
    }
    assert!(compared >= 20, "{compared}");
}

#[test]
fn the_html_report_is_script_free_and_escapes_source_strings() {
    let dir = Temp::new("html");
    std::fs::create_dir_all(dir.0.join("src")).unwrap();
    std::fs::write(
        dir.0.join("src/schema.ts"),
        "import { integer, pgTable, serial, text } from \"drizzle-orm/pg-core\";\nexport const jobs = pgTable(\"jobs\", { id: serial(\"id\").primaryKey(), state: text(\"state\").notNull().default(\"<b>open</b>\") });\nexport const runs = pgTable(\"runs\", { id: serial(\"id\").primaryKey(), jobId: integer(\"job_id\").notNull() });\n",
    )
    .unwrap();
    std::fs::write(
        dir.0.join("src/app.ts"),
        "import { eq } from \"drizzle-orm\";\nimport { jobs, runs } from \"./schema\";\nexport async function start(id: number) {\n  const [job] = await db.select().from(jobs).where(eq(jobs.id, id)).limit(1);\n  if (!job || job.state !== \"<b>open</b>\") { return; }\n  await db.update(jobs).set({ state: \"<script>alert(1)</script>\" }).where(eq(jobs.id, job.id));\n  await db.insert(runs).values({ jobId: job.id });\n}\n",
    )
    .unwrap();
    let output = bin()
        .args(["discover", "--model-check", "--format", "html"])
        .arg(&dir.0)
        .output()
        .unwrap();
    let html = String::from_utf8_lossy(&output.stdout);
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{html}"
    );
    assert!(!html.contains("<script"));
    assert!(html.contains("default-src 'none'"));
}

/// Emit the commerce scenarios once per test process.
fn emitted() -> &'static (Temp, Value) {
    static EMITTED: std::sync::OnceLock<(Temp, Value)> = std::sync::OnceLock::new();
    EMITTED.get_or_init(|| {
        let out = Temp::new("runtime");
        let (_, r) = discover(
            &app(),
            &["--model-check", "--emit-scenarios", out.0.to_str().unwrap()],
        );
        (out, r)
    })
}

fn actual_file(entry: &str, class: &str) -> PathBuf {
    let (out, r) = emitted();
    for c in r["candidates"].as_array().unwrap() {
        let own = c["actorEntries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| c["entries"][i.as_u64().unwrap() as usize]["selector"] == entry);
        if own && c["class"] == class {
            let s = &c["scenarios"][0];
            assert_eq!(s["variant"]["kind"], "actual");
            assert_eq!(
                s["sourceVerification"], true,
                "{entry}: the contracts map it to the harness"
            );
            return out.0.join(s["file"].as_str().unwrap());
        }
    }
    panic!("no {class} candidate of {entry}");
}

#[test]
fn generated_scenarios_never_run_application_code_without_authorization() {
    let file = actual_file(
        "src/app/invitations.ts#inviteMember/arg[1]",
        "check-then-act",
    );
    let (_, r) = model_check(&file, &["--verify-source"]);
    let sv = &r["sourceVerification"];
    assert_eq!(sv["performed"], false);
    assert_eq!(sv["status"], "blocked");
    assert!(
        sv["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.as_str().unwrap().contains("--allow-execution"))
    );
}

#[test]
fn automatically_discovered_counterexamples_reproduce_on_the_original_code() {
    require_source_run!();
    for (entry, class) in [
        (
            "src/app/invitations.ts#inviteMember/arg[1]",
            "check-then-act",
        ),
        ("src/app/inventory.ts#reserveItem", "read-modify-write"),
        ("src/services/orders.ts#claimTask", "status-transition"),
    ] {
        let file = actual_file(entry, class);
        let (code, r) = model_check(&file, &["--verify-source", "--allow-execution"]);
        assert_eq!(code, 1, "{entry}");
        assert_eq!(
            r["exploration"]["verdict"], "COUNTEREXAMPLE_FOUND",
            "{entry}"
        );
        assert_eq!(
            r["evidence"]["level"], "original-code-reproduced",
            "{entry}: {}",
            r["sourceVerification"]
        );
        let sv = &r["sourceVerification"];
        assert_eq!(sv["status"], "reproduced");
        assert!(
            sv["statements"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["status"] == "agree")
        );
    }
}

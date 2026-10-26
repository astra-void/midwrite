//! Runs every file under `fixtures/` (except the multi-file projects in
//! `fixtures/projects/`, see `tests/projects.rs`, the replay harnesses in
//! `fixtures/replay/`, see `tests/traces.rs`, and the model-checking projects in
//! `fixtures/concurrency/`, see `commitrift-cli/tests/model_check.rs`) and
//! checks it against the `// @expect`
//! annotations it contains:
//!
//! ```text
//! // @expect CP1001 line=12 confidence=high function=completeOrder
//! // @expect none
//! // @expect parse-error
//! ```
//!
//! The set of `(code, line)` pairs must match exactly; `confidence` and
//! `function` are checked when given.

use std::path::{Path, PathBuf};

use commitrift_analyzer::{Analyzer, discover, display_path};
use commitrift_core::config::Config;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Expectation {
    code: String,
    line: u32,
    confidence: Option<String>,
    function: Option<String>,
}

#[derive(Debug)]
enum Expected {
    Diagnostics(Vec<Expectation>),
    ParseError,
}

fn expectations(source: &str) -> Result<Expected, String> {
    let mut diagnostics = Vec::new();
    let mut none = false;
    let mut parse_error = false;
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("// @expect") else {
            continue;
        };
        let mut words = rest.split_whitespace();
        match words.next() {
            Some("none") => none = true,
            Some("parse-error") => parse_error = true,
            Some(code) => {
                let mut expectation = Expectation {
                    code: code.to_owned(),
                    line: 0,
                    confidence: None,
                    function: None,
                };
                for word in words {
                    match word.split_once('=') {
                        Some(("line", value)) => {
                            expectation.line =
                                value.parse().map_err(|_| format!("bad line in `{line}`"))?;
                        }
                        Some(("confidence", value)) => {
                            expectation.confidence = Some(value.to_owned())
                        }
                        Some(("function", value)) => expectation.function = Some(value.to_owned()),
                        _ => return Err(format!("unknown expectation field in `{line}`")),
                    }
                }
                if expectation.line == 0 {
                    return Err(format!("missing line= in `{line}`"));
                }
                diagnostics.push(expectation);
            }
            None => return Err(format!("empty expectation `{line}`")),
        }
    }
    match (none, parse_error, diagnostics.is_empty()) {
        (true, false, true) => Ok(Expected::Diagnostics(Vec::new())),
        (false, true, true) => Ok(Expected::ParseError),
        (false, false, false) => Ok(Expected::Diagnostics(diagnostics)),
        (false, false, true) => Err("no `// @expect` annotation".to_owned()),
        _ => Err("conflicting `// @expect` annotations".to_owned()),
    }
}

fn fixture_analyzer() -> Analyzer {
    let config = Config::load(&fixtures_dir().join("commitrift.json")).expect("fixture config");
    Analyzer::new(config)
}

#[test]
fn every_fixture_matches_its_expectations() {
    let analyzer = fixture_analyzer();
    // Multi-file projects are checked as projects by `tests/projects.rs`,
    // replay harnesses by `tests/traces.rs`, model-checking projects by the
    // CLI's `model_check.rs`, the Phase 7 application by `tests/phase7.rs`,
    // the Phase 8 applications by `tests/phase8.rs`, the Phase 9 application
    // by the CLI's `phase9.rs`.
    let files: Vec<PathBuf> = discover(&[fixtures_dir()])
        .expect("fixtures exist")
        .into_iter()
        .filter(|file| {
            !file.starts_with(fixtures_dir().join("projects"))
                && !file.starts_with(fixtures_dir().join("replay"))
                && !file.starts_with(fixtures_dir().join("concurrency"))
                && !file.starts_with(fixtures_dir().join("phase7"))
                && !file.starts_with(fixtures_dir().join("phase8"))
                && !file.starts_with(fixtures_dir().join("phase9"))
        })
        .collect();
    assert!(
        files.len() >= 30,
        "expected the full fixture corpus, found {}",
        files.len()
    );

    let mut problems = Vec::new();
    for file in &files {
        let name = display_path(file.strip_prefix(fixtures_dir()).unwrap_or(file));
        let source = std::fs::read_to_string(file).expect("readable fixture");
        let expected = match expectations(&source) {
            Ok(expected) => expected,
            Err(error) => {
                problems.push(format!("{name}: {error}"));
                continue;
            }
        };
        match (expected, analyzer.analyze_source(&name, &source)) {
            (Expected::ParseError, Err(_)) => {}
            (Expected::ParseError, Ok(_)) => {
                problems.push(format!("{name}: expected a parse error"))
            }
            (Expected::Diagnostics(_), Err(error)) => {
                problems.push(format!("{name}: unexpected parse error: {error}"));
            }
            (Expected::Diagnostics(mut expected), Ok(analysis)) => {
                let mut actual: Vec<(String, u32)> = analysis
                    .diagnostics
                    .iter()
                    .map(|d| (d.code.as_str().to_owned(), d.location.line))
                    .collect();
                actual.sort();
                expected.sort();
                let wanted: Vec<(String, u32)> =
                    expected.iter().map(|e| (e.code.clone(), e.line)).collect();
                if actual != wanted {
                    problems.push(format!("{name}: expected {wanted:?}, got {actual:?}"));
                    continue;
                }
                for expectation in &expected {
                    let diagnostic = analysis
                        .diagnostics
                        .iter()
                        .find(|d| {
                            d.code.as_str() == expectation.code
                                && d.location.line == expectation.line
                        })
                        .expect("matched above");
                    if let Some(confidence) = &expectation.confidence
                        && diagnostic.confidence.to_string() != *confidence
                    {
                        problems.push(format!(
                            "{name}:{}: expected confidence {confidence}, got {}",
                            expectation.line, diagnostic.confidence
                        ));
                    }
                    if let Some(function) = &expectation.function
                        && diagnostic.function != *function
                    {
                        problems.push(format!(
                            "{name}:{}: expected function {function}, got {}",
                            expectation.line, diagnostic.function
                        ));
                    }
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "fixture mismatches:\n{}",
        problems.join("\n")
    );
}

#[test]
fn every_diagnostic_is_self_explanatory() {
    let analyzer = fixture_analyzer();
    for file in discover(&[fixtures_dir()]).expect("fixtures exist") {
        let Ok(analysis) = analyzer.analyze_file(&file) else {
            continue;
        };
        for diagnostic in &analysis.diagnostics {
            let at = format!("{}:{}", diagnostic.location.file, diagnostic.location.line);
            assert!(!diagnostic.message.is_empty(), "{at}: empty message");
            assert!(!diagnostic.mitigation.is_empty(), "{at}: no mitigation");
            assert!(!diagnostic.limitations.is_empty(), "{at}: no limitations");
            assert!(
                !diagnostic.effects.is_empty() || diagnostic.code.as_str() == "CP9001",
                "{at}: no effects"
            );
            assert!(
                !diagnostic.reproduced,
                "{at}: nothing is reproduced in Phase 1"
            );
            assert!(
                diagnostic.location.line > 0 && diagnostic.location.column > 0,
                "{at}: bad location"
            );
            if diagnostic.code.as_str() != "CP9001" {
                assert!(
                    diagnostic.failure_sequence.len() >= 3,
                    "{at}: short failure sequence"
                );
                assert!(!diagnostic.evidence.is_empty(), "{at}: no evidence");
            }
            for effect in &diagnostic.effects {
                assert_eq!(
                    effect.location.file, diagnostic.location.file,
                    "{at}: effect in another file"
                );
            }
        }
    }
}

#[test]
fn scanning_many_files_is_deterministic() {
    let analyzer = fixture_analyzer();
    let run = || {
        let report = analyzer
            .analyze_paths(&[fixtures_dir()], None)
            .expect("fixtures exist");
        serde_json::to_string(&report).expect("serializable")
    };
    let first = run();
    assert_eq!(first, run());

    let report = analyzer.analyze_paths(&[fixtures_dir()], None).unwrap();
    assert_eq!(report.summary.files_failed, 1, "only syntax_error.ts fails");
    assert_eq!(
        report.failures[0].kind,
        commitrift_core::report::FailureKind::Parse
    );
    assert!(
        report.failures[0]
            .file
            .ends_with("unsupported/syntax_error.ts")
    );
    assert_eq!(report.failures[0].line, Some(4));
    let files: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|d| d.location.file.as_str())
        .collect();
    let mut sorted = files.clone();
    sorted.sort_unstable();
    assert_eq!(files, sorted, "diagnostics are ordered by file");
}

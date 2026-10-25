//! Cross-module fixtures: every directory under `fixtures/projects/` is
//! analyzed as one project ([`Analyzer::analyze_project`]), and each of its
//! source files is checked against the `// @expect` annotations it contains
//! (same syntax as `tests/fixtures.rs`). A diagnostic belongs to the file of
//! its primary location.

use std::path::{Path, PathBuf};

use commitrift_analyzer::{Analyzer, discover, display_path};
use commitrift_core::config::Config;

fn projects_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects");
    std::fs::canonicalize(dir).expect("fixtures/projects exists")
}

fn analyzer() -> Analyzer {
    let config = Config::load(&projects_dir().join("../commitrift.json")).expect("fixture config");
    Analyzer::new(config)
}

/// `(code, line, confidence, function)` expectations of one file, or `None`
/// for `// @expect none`.
fn expectations(source: &str) -> Vec<(String, u32, Option<String>, Option<String>)> {
    let mut out = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("// @expect") else {
            continue;
        };
        let mut words = rest.split_whitespace();
        let Some(code) = words.next() else { continue };
        if code == "none" {
            continue;
        }
        let mut expectation = (code.to_owned(), 0, None, None);
        for word in words {
            match word.split_once('=') {
                Some(("line", value)) => expectation.1 = value.parse().expect("line number"),
                Some(("confidence", value)) => expectation.2 = Some(value.to_owned()),
                Some(("function", value)) => expectation.3 = Some(value.to_owned()),
                _ => panic!("unknown expectation field in `{line}`"),
            }
        }
        out.push(expectation);
    }
    out
}

#[test]
fn every_project_fixture_matches_its_expectations() {
    let analyzer = analyzer();
    let mut problems = Vec::new();
    let mut projects: Vec<PathBuf> = std::fs::read_dir(projects_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    projects.sort();
    assert!(projects.len() >= 15, "found {} projects", projects.len());
    for project in &projects {
        let analysis = analyzer
            .analyze_project(std::slice::from_ref(project))
            .expect("project exists");
        assert!(
            analysis.failures.is_empty(),
            "{}: {:?}",
            project.display(),
            analysis.failures
        );
        for file in discover(std::slice::from_ref(project)).unwrap() {
            let source = std::fs::read_to_string(&file).unwrap();
            if !source.contains("// @expect") {
                problems.push(format!("{}: no `// @expect` annotation", file.display()));
                continue;
            }
            let display = display_path(&file);
            let name = display_path(file.strip_prefix(projects_dir()).unwrap());
            let mut expected: Vec<(String, u32)> = expectations(&source)
                .iter()
                .map(|(code, line, ..)| (code.clone(), *line))
                .collect();
            expected.sort();
            let diagnostics: Vec<_> = analysis
                .files
                .iter()
                .flat_map(|f| &f.diagnostics)
                .filter(|d| d.location.file == display)
                .collect();
            let mut actual: Vec<(String, u32)> = diagnostics
                .iter()
                .map(|d| (d.code.as_str().to_owned(), d.location.line))
                .collect();
            actual.sort();
            if actual != expected {
                let details: Vec<String> = diagnostics
                    .iter()
                    .map(|d| {
                        format!(
                            "    {} {}:{} {}",
                            d.code, d.location.line, d.function, d.message
                        )
                    })
                    .collect();
                problems.push(format!(
                    "{name}: expected {expected:?}, got {actual:?}\n{}",
                    details.join("\n")
                ));
                continue;
            }
            for (code, line, confidence, function) in expectations(&source) {
                let d = diagnostics
                    .iter()
                    .find(|d| d.code.as_str() == code && d.location.line == line)
                    .unwrap();
                if let Some(confidence) = confidence
                    && d.confidence.to_string() != confidence
                {
                    problems.push(format!(
                        "{name}:{line}: expected confidence {confidence}, got {}",
                        d.confidence
                    ));
                }
                if let Some(function) = function
                    && d.function != function
                {
                    problems.push(format!(
                        "{name}:{line}: expected function {function}, got {}",
                        d.function
                    ));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "project fixture mismatches:\n{}",
        problems.join("\n")
    );
}

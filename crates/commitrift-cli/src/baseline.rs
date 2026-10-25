//! Baseline files: record the current findings so that CI fails only on new
//! ones.
//!
//! A finding is identified by its fingerprint (code, file, function, and the
//! role and label of each related operation; no line numbers), so edits that
//! move code within a file keep matching. When a file is renamed or a
//! function moves to another file, a second pass matches the remaining
//! findings by the same identity without the file, but only when exactly one
//! baseline entry and exactly one current finding share it.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use commitrift_core::diagnostic::BaselineState;
use commitrift_core::report::{BaselineCounts, Report};

const BASELINE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub version: u32,
    pub tool: String,
    pub findings: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub fingerprint: String,
    /// The fingerprint without the file, for matching moved code.
    pub relocation_key: String,
    pub code: String,
    pub file: String,
    pub function: String,
    pub message: String,
}

impl Baseline {
    pub fn from_report(report: &Report) -> Self {
        Self {
            version: BASELINE_VERSION,
            tool: format!("{} {}", report.tool.name, report.tool.version),
            findings: report
                .diagnostics
                .iter()
                .map(|diagnostic| Entry {
                    fingerprint: diagnostic.fingerprint.clone(),
                    relocation_key: diagnostic.compute_fingerprint(None),
                    code: diagnostic.code.as_str().to_owned(),
                    file: diagnostic.location.file.clone(),
                    function: diagnostic.function.clone(),
                    message: diagnostic.message.clone(),
                })
                .collect(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read baseline `{}`: {error}", path.display()))?;
        let baseline: Self = serde_json::from_str(&text)
            .map_err(|error| format!("invalid baseline `{}`: {error}", path.display()))?;
        if baseline.version != BASELINE_VERSION {
            return Err(format!(
                "baseline `{}` has version {}, expected {BASELINE_VERSION}",
                path.display(),
                baseline.version
            ));
        }
        Ok(baseline)
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        let mut text = serde_json::to_string_pretty(self).expect("baselines are serializable");
        text.push('\n');
        std::fs::write(path, text)
            .map_err(|error| format!("failed to write baseline `{}`: {error}", path.display()))
    }

    /// Mark every diagnostic of `report` as new or existing.
    pub fn apply(&self, report: &mut Report) {
        let mut remaining: HashMap<&str, usize> = HashMap::new();
        for entry in &self.findings {
            *remaining.entry(entry.fingerprint.as_str()).or_default() += 1;
        }
        let mut matched = vec![false; self.findings.len()];
        let mut used: HashMap<&str, usize> = HashMap::new();
        for diagnostic in &mut report.diagnostics {
            let key = diagnostic.fingerprint.as_str();
            let available = remaining.get(key).copied().unwrap_or(0);
            let taken = used.entry(key).or_default();
            if *taken < available {
                *taken += 1;
                diagnostic.baseline = Some(BaselineState::Existing);
            } else {
                diagnostic.baseline = Some(BaselineState::New);
            }
        }
        // Which baseline entries were matched exactly.
        let mut consumed: HashMap<&str, usize> = used;
        for (index, entry) in self.findings.iter().enumerate() {
            if let Some(count) = consumed.get_mut(entry.fingerprint.as_str())
                && *count > 0
            {
                *count -= 1;
                matched[index] = true;
            }
        }

        // Second pass: moved code, matched by the file-independent key when
        // the match is unique on both sides.
        let mut baseline_keys: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, entry) in self.findings.iter().enumerate() {
            if !matched[index] {
                baseline_keys
                    .entry(entry.relocation_key.as_str())
                    .or_default()
                    .push(index);
            }
        }
        let current_keys: Vec<Option<String>> = report
            .diagnostics
            .iter()
            .map(|d| (d.baseline == Some(BaselineState::New)).then(|| d.compute_fingerprint(None)))
            .collect();
        let mut current_count: HashMap<&str, usize> = HashMap::new();
        for key in current_keys.iter().flatten() {
            *current_count.entry(key.as_str()).or_default() += 1;
        }
        for (diagnostic, key) in report.diagnostics.iter_mut().zip(&current_keys) {
            let Some(key) = key else { continue };
            if current_count.get(key.as_str()) != Some(&1) {
                continue;
            }
            if let Some(indices) = baseline_keys.get(key.as_str())
                && let [index] = indices[..]
                && !matched[index]
            {
                matched[index] = true;
                diagnostic.baseline = Some(BaselineState::Existing);
            }
        }

        let existing = report
            .diagnostics
            .iter()
            .filter(|d| d.baseline == Some(BaselineState::Existing))
            .count();
        report.summary.baseline = Some(BaselineCounts {
            new: report.diagnostics.len() - existing,
            existing,
            resolved: matched.iter().filter(|m| !**m).count(),
        });
    }
}

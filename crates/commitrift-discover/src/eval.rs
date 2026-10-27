//! Scoring a discovery report against labeled entries
//! (`fixtures/phase11/labels.json`).
//!
//! The measurement problems are kept apart (`docs/phase11/PRECISION-RECALL.md`):
//!
//! * **static hazard prediction**: an entry has a candidate of its own (its
//!   actors are only this entry) that is not blocked and has no recognized
//!   protection;
//! * **model prediction**: an entry has a candidate of its own whose actual
//!   scenario has a validated model counterexample;
//! * **class agreement**: for a predicted positive, whether a candidate of
//!   the labeled class is among the ones with a counterexample;
//! * **unsupported handling**: an entry labeled `unsupported` is reported
//!   with unknown operations (or a failed extraction) and no candidate of it
//!   claims a bounded safe result.
//!
//! Precision and recall are computed over entries labeled `positive` or
//! `negative` only.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::report::DiscoveryReport;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Labels {
    pub cases: Vec<Case>,
    #[serde(default)]
    pub corrections: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Case {
    pub entry: String,
    pub class: String,
    pub label: String,
    #[serde(default)]
    pub property: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Confusion {
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    pub true_negatives: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recall: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub f1: Option<f64>,
}

impl Confusion {
    fn add(&mut self, predicted: bool, actual: bool) {
        match (predicted, actual) {
            (true, true) => self.true_positives += 1,
            (true, false) => self.false_positives += 1,
            (false, true) => self.false_negatives += 1,
            (false, false) => self.true_negatives += 1,
        }
    }

    fn finish(&mut self) {
        let tp = self.true_positives as f64;
        let p = tp + self.false_positives as f64;
        let r = tp + self.false_negatives as f64;
        self.precision = (p > 0.0).then(|| round(tp / p));
        self.recall = (r > 0.0).then(|| round(tp / r));
        if let (Some(p), Some(r)) = (self.precision, self.recall)
            && p + r > 0.0
        {
            self.f1 = Some(round(2.0 * p * r / (p + r)));
        }
    }
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseResult {
    pub entry: String,
    pub label: String,
    pub class: String,
    /// Statuses of the entry's own candidates, `class:STATUS`.
    pub candidates: Vec<String>,
    pub static_prediction: bool,
    pub model_prediction: bool,
    /// For a model-predicted positive: a candidate of the labeled class has
    /// the counterexample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_agrees: Option<bool>,
    /// `TP`, `FP`, `FN`, `TN`, or for `unsupported` labels `reported` /
    /// `missed`.
    pub outcome: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Evaluation {
    pub cases: Vec<CaseResult>,
    pub static_hazard_prediction: Confusion,
    pub model_prediction: Confusion,
    pub class_agreement: BTreeMap<String, usize>,
    pub unsupported_reported: usize,
    pub unsupported_missed: usize,
    /// Of the entries with a candidate whose scenario was satisfiable, how
    /// many got a model result.
    pub candidate_to_model: String,
    pub labels_corrected: usize,
}

pub fn evaluate(report: &DiscoveryReport, labels: &Labels) -> Evaluation {
    let mut cases = Vec::new();
    let mut stat = Confusion::default();
    let mut model = Confusion::default();
    let mut agreement: BTreeMap<String, usize> = BTreeMap::new();
    let (mut reported, mut missed) = (0, 0);
    for case in &labels.cases {
        let own: Vec<_> = report
            .candidates
            .iter()
            .filter(|c| {
                c.candidate
                    .actor_entries
                    .iter()
                    .all(|&i| c.candidate.entries[i].selector == case.entry)
            })
            .collect();
        let static_prediction = own
            .iter()
            .any(|c| c.candidate.blocked.is_none() && c.candidate.protections.is_empty());
        let with_ce: Vec<_> = own
            .iter()
            .filter(|c| c.status == "MODEL_COUNTEREXAMPLE")
            .collect();
        let model_prediction = !with_ce.is_empty();
        let entry = report.entries.iter().find(|e| e.selector == case.entry);
        let statuses: Vec<String> = own
            .iter()
            .map(|c| format!("{}:{}", c.candidate.class.as_str(), c.status))
            .collect();
        let (outcome, class_agrees, note) = match case.label.as_str() {
            "positive" | "negative" => {
                let actual = case.label == "positive";
                stat.add(static_prediction, actual);
                model.add(model_prediction, actual);
                let agrees = model_prediction.then(|| {
                    with_ce
                        .iter()
                        .any(|c| c.candidate.class.as_str() == case.class)
                });
                if let Some(a) = agrees {
                    *agreement
                        .entry(if a { "agrees" } else { "differs" }.to_owned())
                        .or_default() += 1;
                }
                let outcome = match (model_prediction, actual) {
                    (true, true) => "TP",
                    (true, false) => "FP",
                    (false, true) => "FN",
                    (false, false) => "TN",
                };
                let note = if outcome == "FP" {
                    with_ce
                        .iter()
                        .map(|c| c.finding.clone())
                        .collect::<Vec<_>>()
                        .join(" | ")
                } else {
                    String::new()
                };
                (outcome.to_owned(), agrees, note)
            }
            _ => {
                let unknown =
                    entry.is_some_and(|e| !e.unknown.is_empty() || e.status == "extraction-failed");
                let claims_safe = own.iter().any(|c| c.status == "NO_VIOLATION_WITHIN_BOUNDS");
                if unknown && !claims_safe {
                    reported += 1;
                    ("reported".to_owned(), None, String::new())
                } else {
                    missed += 1;
                    (
                        "missed".to_owned(),
                        None,
                        format!("entry status {:?}", entry.map(|e| e.status.clone())),
                    )
                }
            }
        };
        cases.push(CaseResult {
            entry: case.entry.clone(),
            label: case.label.clone(),
            class: case.class.clone(),
            candidates: statuses,
            static_prediction,
            model_prediction,
            class_agrees,
            outcome,
            note,
        });
    }
    stat.finish();
    model.finish();
    let satisfiable = report
        .candidates
        .iter()
        .filter(|c| {
            c.scenarios
                .iter()
                .any(|s| s.synthesis.status == "satisfied" && !s.invariants.is_empty())
        })
        .count();
    let checked = report
        .candidates
        .iter()
        .filter(|c| c.scenarios.iter().any(|s| s.model.is_some()))
        .count();
    Evaluation {
        cases,
        static_hazard_prediction: stat,
        model_prediction: model,
        class_agreement: agreement,
        unsupported_reported: reported,
        unsupported_missed: missed,
        candidate_to_model: format!("{checked}/{satisfiable}"),
        labels_corrected: labels.corrections.len(),
    }
}

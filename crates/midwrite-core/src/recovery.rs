//! Recovery contracts: what the configuration asserts about recovery
//! mechanisms *outside* an analyzed function.
//!
//! A structural failure window is not always a bug: a job runner may
//! re-execute the whole function after an error or a crash, or a
//! reconciliation process may repair the state later. Midwrite cannot see
//! those mechanisms (they live in queue configuration, other services,
//! operations), and it never infers them from names: a function called
//! `retryFailedJobs` proves nothing. A contract is an explicit, per-function
//! assertion that must cite its evidence. Its effect is deliberately
//! limited:
//!
//! * the finding is still reported, with the same fingerprint;
//! * under the `recommended` policy a covered warning becomes informational;
//!   `strict` and `audit` only attach the note;
//! * the note records its provenance (`configuration`, `verified: false`):
//!   it is never shown as runtime evidence. Only a replay harness's recovery
//!   scenario can observe a recovery (see `verification::RecoveryObservation`).
//!
//! See `docs/concepts/failure-model.md`.

use serde::{Deserialize, Serialize};

use crate::diagnostic::{
    Diagnostic, DiagnosticCode, Policy, RecoveryDeclaration, RecoveryNote, Severity,
};

/// How the application asserts a covered window is repaired.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContractStrategy {
    /// A mechanism outside the function (a job runner's retries and
    /// stalled-job recovery, a client retry, webhook redelivery) executes the
    /// whole function again after it failed or its process died, until it
    /// completes or the mechanism gives up. Repairs a missing second effect
    /// by doing everything again, so the first effect is repeated (it must be
    /// idempotent, or its duplicates acceptable).
    Retried,
    /// A separate process derives missing effects from committed database
    /// state and performs them. Cannot undo an external effect whose database
    /// change never committed (MW1002, MW1003).
    Reconciled,
}

impl ContractStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Retried => "retried",
            Self::Reconciled => "reconciled",
        }
    }

    /// The diagnostic codes whose failure window the strategy can repair.
    pub fn can_cover(self, code: DiagnosticCode) -> bool {
        use DiagnosticCode::*;
        match self {
            Self::Retried => matches!(
                code,
                DbCommitThenExternal
                    | ExternalThenDbCommit
                    | ExternalInsideTransaction
                    | IndependentDbCommits
                    | IndependentExternalEffects
            ),
            Self::Reconciled => matches!(
                code,
                DbCommitThenExternal | IndependentDbCommits | IndependentExternalEffects
            ),
        }
    }
}

/// `recoveryContracts[]` in `midwrite.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryContract {
    /// The function as diagnostics name it (`completeOrder`,
    /// `ProcessDocumentProcessor.process`). Exact match; no patterns.
    pub function: String,
    /// The file as diagnostics show it (relative to the analysis root).
    /// Without it the contract applies to every function of that name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub strategy: ContractStrategy,
    /// The diagnostic codes the contract covers (explicit; at least one).
    pub covers: Vec<String>,
    /// Where the mechanism is: configuration lines, documentation, the job
    /// definition. Shown with every covered finding.
    pub evidence: String,
    /// What the mechanism does not do (bounded attempts, dead-letter queue,
    /// manual repair after N failures).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<String>,
}

pub fn validate_contracts(contracts: &[RecoveryContract]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for (index, contract) in contracts.iter().enumerate() {
        let at = format!("recoveryContracts[{index}]");
        if contract.function.trim().is_empty() || contract.function.contains(['*', '?', ' ']) {
            return Err(format!(
                "{at}: `function` must name one function exactly as diagnostics show it (no patterns)"
            ));
        }
        if contract.file.as_deref().is_some_and(str::is_empty) {
            return Err(format!("{at}: empty `file`"));
        }
        if contract.evidence.trim().len() < 10 {
            return Err(format!(
                "{at}: `evidence` must say where the recovery mechanism is (configuration, job definition, documentation); a contract without evidence is not accepted"
            ));
        }
        if contract.covers.is_empty() {
            return Err(format!(
                "{at}: `covers` must list the diagnostic codes the mechanism repairs"
            ));
        }
        for code in &contract.covers {
            let parsed: DiagnosticCode = code.parse().map_err(|e| format!("{at}: covers: {e}"))?;
            if !contract.strategy.can_cover(parsed) {
                return Err(format!(
                    "{at}: a `{}` contract cannot cover {code}{}",
                    contract.strategy.as_str(),
                    match parsed {
                        DiagnosticCode::AnalysisIncomplete => ": MW9001 is an analysis limitation, not a failure window".to_owned(),
                        _ => ": reconciliation from database state cannot undo an external effect whose database change never committed".to_owned(),
                    }
                ));
            }
        }
        if !seen.insert((contract.function.as_str(), contract.file.as_deref())) {
            return Err(format!(
                "{at}: a contract for `{}` is declared twice",
                contract.function
            ));
        }
    }
    Ok(())
}

fn describe(contract: &RecoveryContract, code: DiagnosticCode) -> String {
    let what = match contract.strategy {
        ContractStrategy::Retried => {
            let repeat = match code {
                DiagnosticCode::IndependentDbCommits => {
                    "the writes that committed before the failure run again, so they must be idempotent"
                }
                _ => {
                    "the effect that completed before the failure is performed again, so it must be idempotent or its duplicates acceptable"
                }
            };
            format!(
                "a mechanism outside `{}` is asserted to execute it again after an error or a process crash; {repeat}",
                contract.function
            )
        }
        ContractStrategy::Reconciled => format!(
            "a process outside `{}` is asserted to derive the missing effect from committed database state",
            contract.function
        ),
    };
    format!(
        "The configuration declares a `{}` recovery contract for `{}` ({what}). Cited evidence: {}.{} Midwrite does not verify that the mechanism exists, covers this window or runs after a crash; the window is still reported.",
        contract.strategy.as_str(),
        contract.function,
        contract.evidence.trim_end_matches('.'),
        contract
            .limits
            .as_deref()
            .map(|l| format!(" Declared limits: {}.", l.trim_end_matches('.')))
            .unwrap_or_default()
    )
}

/// Attach the matching contract to each covered diagnostic and, under the
/// `recommended` policy, make a covered warning informational. Fingerprints,
/// codes, locations and traces are unchanged.
pub fn apply_contracts(
    diagnostics: &mut [Diagnostic],
    contracts: &[RecoveryContract],
    policy: Policy,
) {
    if contracts.is_empty() {
        return;
    }
    for diagnostic in diagnostics {
        let code = diagnostic.code;
        let Some(contract) = contracts.iter().find(|c| {
            c.function == diagnostic.function
                && c.file
                    .as_deref()
                    .is_none_or(|file| file == diagnostic.location.file)
                && c.covers.iter().any(|covered| covered == code.as_str())
        }) else {
            continue;
        };
        let lowered = policy == Policy::Recommended && diagnostic.severity == Severity::Warning;
        if lowered {
            diagnostic.severity = Severity::Info;
        }
        diagnostic.recovery.push(RecoveryNote {
            effect: diagnostic.function.clone(),
            declaration: match contract.strategy {
                ContractStrategy::Retried => RecoveryDeclaration::Retried,
                ContractStrategy::Reconciled => RecoveryDeclaration::Reconciled,
            },
            source: "configuration",
            verified: false,
            effect_on_finding: if lowered { "informational" } else { "noteOnly" },
            scope: "function",
            evidence: Some(contract.evidence.clone()),
            limits: contract.limits.clone(),
        });
        diagnostic.limitations.push(describe(contract, code));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(json: &str) -> Result<Vec<RecoveryContract>, String> {
        let contracts: Vec<RecoveryContract> =
            serde_json::from_str(json).map_err(|e| e.to_string())?;
        validate_contracts(&contracts)?;
        Ok(contracts)
    }

    #[test]
    fn contracts_must_be_explicit_cite_evidence_and_fit_their_strategy() {
        contract(
            r#"[{ "function": "Job.process", "file": "src/job.ts", "strategy": "retried", "covers": ["MW1001", "MW1002"], "evidence": "queue.config.ts:12 attempts 3" }]"#,
        )
        .unwrap();
        for bad in [
            r#"[{ "function": "*", "strategy": "retried", "covers": ["MW1001"], "evidence": "queue.config.ts:12" }]"#,
            r#"[{ "function": "f", "strategy": "retried", "covers": ["MW1001"], "evidence": "" }]"#,
            r#"[{ "function": "f", "strategy": "retried", "covers": [], "evidence": "queue.config.ts:12" }]"#,
            r#"[{ "function": "f", "strategy": "retried", "covers": ["MW9001"], "evidence": "queue.config.ts:12" }]"#,
            r#"[{ "function": "f", "strategy": "reconciled", "covers": ["MW1002"], "evidence": "cron.ts:3 nightly" }]"#,
            r#"[{ "function": "f", "strategy": "retried", "covers": ["MW7777"], "evidence": "queue.config.ts:12" }]"#,
            r#"[{ "function": "f", "strategy": "inferred", "covers": ["MW1001"], "evidence": "queue.config.ts:12" }]"#,
            r#"[{ "function": "f", "strategy": "retried", "covers": ["MW1001"], "evidence": "queue.config.ts:12", "auto": true }]"#,
            r#"[{ "function": "f", "strategy": "retried", "covers": ["MW1001"], "evidence": "queue.config.ts:12" },
                { "function": "f", "strategy": "reconciled", "covers": ["MW1001"], "evidence": "queue.config.ts:12" }]"#,
        ] {
            assert!(contract(bad).is_err(), "{bad}");
        }
    }
}

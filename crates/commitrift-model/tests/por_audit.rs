//! The independence audit (Phase 10) on the whole corpus, and its power: with
//! a kind of access deleted from the footprints the audit must notice, on at
//! least one workload, the dependencies the reductions rely on.

mod common;

use commitrift_model::audit::{ViolationKind, audit};
use commitrift_model::footprint::{Access, mutation};
use commitrift_model::model::Model;
use common::adversarial::generate;
use common::por_workloads::{dpor_model, phase9_workloads, targeted_workloads};

const MAX_STATES: usize = 30_000;

fn corpus() -> Vec<(String, Model)> {
    let mut out: Vec<(String, Model)> = targeted_workloads()
        .into_iter()
        .map(|(l, m)| (l.to_owned(), m))
        .collect();
    out.extend(phase9_workloads());
    for seed in 1..=90 {
        let g = generate(seed);
        let model = commitrift_model::scenario::load_str(
            &g.doc.to_string(),
            std::path::Path::new("."),
            &commitrift_model::provider::NoProvider,
        )
        .expect("generated scenarios load")
        .0;
        out.push((format!("adversarial {seed}"), model));
    }
    for seed in 1..=30 {
        out.push((format!("random {seed}"), dpor_model(seed).1));
    }
    out
}

#[test]
fn the_footprints_of_every_corpus_workload_pass_the_audit() {
    let (mut pairs, mut enabling, mut co, mut drift) = (0, 0, 0, 0);
    for (label, model) in corpus() {
        let rep = audit(&model, MAX_STATES, 3);
        assert!(
            rep.violations.is_empty(),
            "{label}: {:?} ({})",
            rep.violations[0],
            rep.violations[0].kind.describe()
        );
        pairs += rep.independent_pairs;
        enabling += rep.enabling_pairs;
        co += rep.co_enabling_pairs;
        drift += rep.footprint_drift;
    }
    eprintln!(
        "audited: {pairs} independent pairs, {enabling} enabling pairs, {co} co-enabling pairs, {drift} footprint drifts"
    );
    assert!(pairs > 1000 && enabling > 100);
}

/// The audit finds violations when a kind of access is deleted: which kinds it
/// notices is printed; the pool claim and the commutation-carrying kinds must
/// be among them.
#[test]
fn the_audit_notices_deleted_dependencies() {
    let corpus = corpus();
    let mut table = String::new();
    let mut noticed: Vec<&str> = Vec::new();
    for (name, bit) in Access::KINDS {
        let mut hits = 0;
        let mut kinds = std::collections::BTreeSet::new();
        for (_, model) in &corpus {
            mutation::mute(bit);
            let rep = audit(model, 5_000, 1);
            mutation::mute(0);
            if let Some(v) = rep.violations.first() {
                hits += 1;
                kinds.insert(match v.kind {
                    ViolationKind::EnablingPairIndependent => "enabling",
                    _ => "commutation",
                });
            }
        }
        table.push_str(&format!("{name}: {hits} workloads ({kinds:?})\n"));
        if hits > 0 {
            noticed.push(name);
        }
    }
    // The Phase 9 defect, exactly: the claim of a handed-over connection.
    let mut claim_hits = 0;
    for (_, model) in &corpus {
        mutation::mute_claim_pool(true);
        let rep = audit(model, 5_000, 1);
        mutation::mute_claim_pool(false);
        if rep
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::EnablingPairIndependent)
        {
            claim_hits += 1;
        }
    }
    eprintln!("{table}claim-lease pool access deleted: {claim_hits} workloads");
    assert!(
        claim_hits > 0,
        "the audit does not see the Phase 9 pool hand-over defect"
    );
    for must in ["Pred", "RowWrite", "Global"] {
        assert!(noticed.contains(&must), "{must} not noticed: {table}");
    }
}

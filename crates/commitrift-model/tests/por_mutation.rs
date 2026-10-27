//! Mutation testing of the dependence relation.
//!
//! Every reduction trusts the footprints: two transitions whose footprints do
//! not conflict are treated as independent. Phase 9 found a missing access
//! (the pool hand-over) only in the finalization audit, because the random
//! scenario generator had no pools. This test deletes one *kind* of access
//! from every footprint (`footprint::mutation`) and asks, for every
//! algorithm, whether some workload of the corpus then loses a terminal
//! state, a verdict or a stuck state relative to the unreduced search (which
//! does not use footprints). A mutant that no workload kills means the corpus
//! cannot tell that dependency from none; that is reported, not hidden.
//!
//! What the test asserts is narrow and deliberate: the table is printed, the
//! unmutated run must agree everywhere, and the algorithms that carry the
//! enabling edges must not be killed by the pool mutant that killed the
//! classic search in Phase 9.

mod common;

use commitrift_model::explore::SdporConfig;
use commitrift_model::footprint::{Access, mutation};
use commitrift_model::model::Model;
use common::por_diff::{Alg, discrepancy, full, run};
use common::por_workloads::{dpor_model, phase9_workloads, targeted_workloads};

fn corpus() -> Vec<(String, Model)> {
    let mut out: Vec<(String, Model)> = targeted_workloads()
        .into_iter()
        .map(|(l, m)| (l.to_owned(), m))
        .collect();
    out.extend(phase9_workloads());
    // Fewer random seeds in a debug build (each mutant runs seven searches).
    let random = if cfg!(debug_assertions) { 25 } else { 60 };
    for seed in 1..=random {
        out.push((format!("seed {seed}"), dpor_model(seed).1));
    }
    out
}

/// The Phase 9 search as the new explorer expresses it: last race only, no
/// enabling rule. (The classic search itself now handles every race.)
const PHASE9_LIKE: SdporConfig = SdporConfig {
    no_enabling_edges: true,
    defects: commitrift_model::explore::DEFECT_LAST_RACE_ONLY,
    ..SdporConfig::STATELESS
};

const ALGS: [Alg; 7] = [
    Alg::Sdpor(PHASE9_LIKE),
    Alg::Sleep,
    Alg::Classic,
    Alg::Sdpor(SdporConfig::STATEFUL),
    Alg::Sdpor(SdporConfig::STATELESS_SLEEP),
    Alg::Sdpor(SdporConfig::STATEFUL_SLEEP),
    Alg::Sdpor(SdporConfig {
        no_enabling_edges: true,
        ..SdporConfig::STATEFUL_SLEEP
    }),
];

/// For one mutation mask: per algorithm, the labels of the workloads that
/// detect it.
fn killed_by(
    mask: u32,
    corpus: &[(String, Model)],
    fulls: &[Option<commitrift_model::explore::ExploreResult>],
) -> Vec<Vec<String>> {
    let mut out = vec![Vec::new(); ALGS.len()];
    for ((label, model), full) in corpus.iter().zip(fulls) {
        let Some(full) = full else { continue };
        for (i, alg) in ALGS.iter().enumerate() {
            mutation::mute(mask);
            let r = run(*alg, model);
            mutation::mute(0);
            if r.truncation.max_states || r.truncation.max_transitions {
                continue;
            }
            if discrepancy(full, &r, model).is_some() {
                out[i].push(label.clone());
            }
        }
    }
    out
}

#[test]
fn which_missing_dependencies_does_the_corpus_notice() {
    let corpus = corpus();
    let fulls: Vec<_> = corpus.iter().map(|(_, m)| full(m)).collect();
    // The unmutated searches agree everywhere (the premise).
    for ((label, model), f) in corpus.iter().zip(&fulls) {
        let Some(f) = f else { continue };
        for alg in ALGS {
            let r = run(alg, model);
            assert_eq!(discrepancy(f, &r, model), None, "{label}: {}", alg.name());
        }
    }
    let mut table = String::from("mutant (access kind deleted)");
    for a in ALGS {
        table.push_str(&format!(" | {}", a.name()));
    }
    table.push('\n');
    for (name, bit) in Access::KINDS {
        let kills = killed_by(bit, &corpus, &fulls);
        table.push_str(name);
        for k in &kills {
            table.push_str(&format!(" | {}", k.len()));
        }
        table.push('\n');
    }
    eprintln!("workloads that notice each mutant:\n{table}");

    // The Phase 9 defect itself: the step that takes over a handed-over
    // connection recorded no pool access, so happens-before did not order it
    // after the release. The classic search then missed terminal states of
    // the pooled workloads. With the enabling edges the claim is ordered
    // after the release whatever the footprints say.
    let mut claim = String::new();
    let mut killed = vec![Vec::new(); ALGS.len()];
    for ((label, model), f) in corpus.iter().zip(&fulls) {
        let Some(f) = f else { continue };
        for (i, alg) in ALGS.iter().enumerate() {
            mutation::mute_claim_pool(true);
            let r = run(*alg, model);
            mutation::mute_claim_pool(false);
            if discrepancy(f, &r, model).is_some() {
                killed[i].push(label.clone());
            }
        }
    }
    for (a, k) in ALGS.iter().zip(&killed) {
        claim.push_str(&format!("  {:32} killed by {:?}\n", a.name(), k));
    }
    eprintln!("claim-lease pool access deleted (the Phase 9 defect):\n{claim}");
    let idx = |name: &str| ALGS.iter().position(|a| a.name() == name).unwrap();
    assert!(
        !killed[idx("sdpor[stateless,no-edges]")].is_empty(),
        "the corpus no longer reproduces the Phase 9 pool defect on the Phase 9 search"
    );
    for name in [
        "sdpor[cache]",
        "sdpor[stateless+sleep]",
        "sdpor[cache+sleep]",
    ] {
        assert!(
            killed[idx(name)].is_empty(),
            "{name} lost terminal states without the claim access: {:?}",
            killed[idx(name)]
        );
    }
}

/// The same table over the adversarial corpus: which deleted dependencies
/// does a generated workload notice? This is the measure of the generator's
/// power (a kind no workload kills cannot be tested for).
#[test]
fn which_missing_dependencies_does_the_generated_corpus_notice() {
    use common::adversarial::generate;
    let n: u64 = std::env::var("COMMITRIFT_MUT_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) { 90 } else { 270 });
    let mut corpus: Vec<(String, Model)> = Vec::new();
    for seed in 1..=n {
        let g = generate(seed);
        let Ok((m, _)) = commitrift_model::scenario::load_str(
            &g.doc.to_string(),
            std::path::Path::new("."),
            &commitrift_model::provider::NoProvider,
        ) else {
            continue;
        };
        corpus.push((format!("adversarial {seed}"), m));
    }
    let fulls: Vec<_> = corpus.iter().map(|(_, m)| full(m)).collect();
    let algs = [
        Alg::Sleep,
        Alg::Sdpor(SdporConfig::STATEFUL),
        Alg::Sdpor(SdporConfig::STATEFUL_SLEEP),
    ];
    let mut table =
        String::from("mutant | killed by (sleep, stateful, stateful+sleep) | example\n");
    for (name, bit) in Access::KINDS {
        let mut kills = [0u32; 3];
        let mut example = None;
        for ((label, model), f) in corpus.iter().zip(&fulls) {
            let Some(f) = f else { continue };
            if f.metrics.transitions > 4000 {
                continue;
            }
            for (i, alg) in algs.iter().enumerate() {
                mutation::mute(bit);
                let r = run(*alg, model);
                mutation::mute(0);
                if r.truncation.max_states || r.truncation.max_transitions {
                    continue;
                }
                if discrepancy(f, &r, model).is_some() {
                    kills[i] += 1;
                    example.get_or_insert_with(|| label.clone());
                }
            }
        }
        table.push_str(&format!(
            "{name} | {kills:?} | {}\n",
            example.unwrap_or_default()
        ));
    }
    eprintln!("deleted access kind noticed by generated workloads:\n{table}");
}

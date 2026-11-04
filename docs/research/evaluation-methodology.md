# Evaluation methodology

How CommitRift's results are measured, what each kind of measurement can
support, and the rules every phase report follows. The measured results
themselves are in the [evidence index](evidence-index.md).

## Separate questions, separate measurements

| question | measured by | cannot be answered by |
| --- | --- | --- |
| Are static findings real windows, and are they bugs? (static-analysis accuracy) | manual or blind review of corpus findings against source | model checking or runtime replay of fixtures |
| Does discovery find the labeled hazards and only them? (candidate-discovery accuracy) | frozen labeled applications, TP/FP/FN/TN of model prediction and of static prediction | corpus runs without labels |
| Does the search find every reachable behavior of the model? (model-checking correctness) | reductions compared with the unreduced search on targeted, random, adversarial and metamorphic models; independence audit; mutation of the dependence relation | PostgreSQL runs |
| Does the model behave like PostgreSQL? (PostgreSQL differential fidelity) | lockstep replay of every schedule of a workload on PostgreSQL; independent ground truth replayed in the model (Phase 13) | original-code runs alone |
| Does the original code behave like the model? (original-code reproduction) | runs of the unchanged first-party application under the counterexample's schedule | model or SQL replay |
| How fast and how large? (performance) | same machine, same inputs, previous and new binary, medians and peak RSS | runs on different days or machines |

A phase report states each separately; one never stands in for another.

## Rules

1. **Every number comes from a file under `reports/phaseN/`**, produced
   by a named binary or source digest. Nothing is estimated unless the
   report says so.
2. **One revision per result.** Results of different binaries are never
   combined into one figure. Comparisons name both binaries (the previous
   phase's release binary is preserved for this, for example
   `reports/phase13/baseline/commitrift-phase12`, ignored by git).
3. **Baselines first.** Each phase starts by re-running the previous
   phase's gates on the committed tree (in Phase 13 on a copy in a private
   scratch directory with its own `CARGO_TARGET_DIR`, because a baseline
   run in the working tree can compile early edits).
4. **Held-out means held out.** A labeled set scored once before any change
   it prompts is reported as the first scoring; scores after changes it
   prompted are reported separately as post-fix or post-feedback, never as
   held-out results.
5. **Frozen labels.** Since Phase 12, evaluation applications and labels
   are frozen with SHA-256 checksums before the first scoring
   (`reports/phase12/eval/FROZEN.md`, `fixtures/phase13/FROZEN.json`);
   scoring scripts verify the checksums; labels are never edited, and
   corrections go into a separate `corrections` array with a reason.
6. **Unsupported is reported, not scored away.** Entries labeled
   unsupported must be reported as unsupported; they are never counted as
   safe.
7. **Divergence is reported.** A runtime run that violates the property
   along a different schedule is `diverged`, not a reproduction; failed and
   blocked runs are kept in the reports (for example Phase 8's
   deleted-dependencies run, Phase 10's Docker-not-running log).
8. **Compatibility is checked.** Static corpus outputs (144 combinations)
   and the Phase 6–9 model-check scenarios are compared byte for byte with
   the previous binary; differences are classified.
9. **Privacy.** Private corpus projects appear only as aggregate counts and
   generic causes.

## Independence

**No independent human review has been performed.** The repository's
labels, reviews and evaluation applications were written by the
implementer or by AI sessions. The levels of separation that do exist:

| phase | who labeled | separation from the implementation |
| --- | --- | --- |
| 2 | the Phase 2 implementer | none; from source code alone |
| 3 | a separate automated session (blind re-review of 18 findings) | blind to the earlier labels; not a human |
| 11 | the implementer (first-party set); earlier phases' fixtures (held-out set) | none for the first-party set; held-out only for its first scoring |
| 12 | the implementer, fresh application frozen before scoring | none |
| 13 | an AI agent without access to `crates/` or the in-progress Phase 13 code; it read Phase 12 documents and fixtures | independent of the code, not of the project; fresh, not blind |
| 14–15 | AI agents without `crates/` access, as in Phase 13 | as above |
| 16 | an AI agent that did not read `crates/`, `runtime/`, `docs/phase16/` or earlier design documents and ran no build or CommitRift command; it read the user-facing reference docs (supported patterns, limitations) and the Phase 15 set's format | independent of the code, not blind to the documented subset; frozen and scored once before feedback |

AI evaluation is not review. Statements in these documents that a result
is "verified" refer to the measurements above, never to human review.

## Corpus

Two public projects at pinned revisions (saas-starter `6e33e58b`, midday
`51587319`) and two private projects (local copies with recorded local
changes) are analyzed statically only: `corpora/fetch.sh` clones the public
ones into `.corpora/` (ignored by git); manifests and per-project
configurations are under `corpora/`. No corpus project has ever been
executed. Procedure: [corpus evaluation](../development/corpus-evaluation.md).

## Runtime evidence procedure

Original-code evidence is produced by phase scripts
(`scripts/phase8-evidence.py` … `scripts/phase13-evidence.py`) that run
each scenario N times (Phases 8–10: 5 on macOS, 3 in the Linux container;
Phases 11–13: 3) and exit non-zero unless every scenario ran N times with
the expected outcome (since a Phase 8 fix; Phase 12's script exits 1 by
design because two scenarios diverge). Reports are checked against their
JSON Schemas (`scripts/validate-schemas.py`, and the Phase 9/10 report
checkers with deliberate corruptions). Linux runs use a
Debian 12 container on the same machine
(`scripts/phase{8,9,10,12,13}-linux-docker.sh`) with the source digest
compared to the macOS tree.

## Known weaknesses of the methodology

* Labeled sets are small (15 entries in Phase 12, 27 in Phase 13) and
  written by people or agents who knew the supported subset.
* First-party fixtures were written to contain the patterns they test;
  their reproductions are evidence about the patterns, not about other
  applications.
* Two PostgreSQL versions (17.11, 15.19), one architecture (arm64), one
  physical machine.
* No maintainer confirmation and no external application execution.

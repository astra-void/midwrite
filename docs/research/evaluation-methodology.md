# Evaluation methodology

How Midwrite's results are measured, what each kind of measurement can
support, and the rules every evaluation follows. The measured results
themselves are in the [evidence index](evidence-index.md); the labeled
sets are described in [evaluation datasets](evaluation-datasets.md).

## Separate questions, separate measurements

| question | measured by | cannot be answered by |
| --- | --- | --- |
| Are static findings real windows, and are they bugs? (static-analysis accuracy) | manual or blind review of corpus findings against source; audited known positives on dev and held-out projects ([evaluation harness](../development/evaluation-harness.md)) | model checking or runtime replay of fixtures |
| Does discovery find the labeled hazards and only them? (candidate-discovery accuracy) | frozen labeled applications, TP/FP/FN/TN of model prediction and of static prediction ([datasets § scoring](evaluation-datasets.md#scoring)) | corpus runs without labels |
| Does the search find every reachable behavior of the model? (model-checking correctness) | reductions compared with the unreduced search on targeted, random, adversarial and metamorphic models; independence audit; mutation of the dependence relation | PostgreSQL runs |
| Does the model behave like PostgreSQL? (PostgreSQL differential fidelity) | lockstep replay of every schedule of a workload on PostgreSQL; an independent ground truth (the lock matrix, `fixtures/model/lock-matrix`) replayed in the model | original-code runs alone |
| Does the original code behave like the model? (original-code reproduction) | runs of the unchanged first-party application under the counterexample's schedule | model or SQL replay |
| How fast and how large? (performance) | same machine, same inputs, previous and new binary, medians and peak RSS ([benchmarks](../development/benchmarks.md)) | runs on different days or machines |

Every evaluation states each separately; one never stands in for another.
No single accuracy number is reported.

## Rules

1. **Every number comes from a recorded file**, produced by a named binary
   or source digest. Nothing is estimated unless the record says so.
   Current records live under `reports/eval/` and the test inputs under
   `fixtures/reports/`; the per-phase records of the development history
   are archived ([history](../HISTORY.md)).
2. **One revision per result.** Results of different binaries are never
   combined into one figure. Comparisons name both binaries (the previous
   release binary is preserved for this outside version control).
3. **Baselines first.** Before a change is measured, the previous gates are
   re-run on the committed tree — on a copy in a private scratch directory
   with its own `CARGO_TARGET_DIR`, because a baseline run in the working
   tree can compile early edits.
4. **Held-out means held out.** A labeled set scored once before any change
   it prompts is reported as the first scoring; scores after changes it
   prompted are reported separately as post-fix or post-feedback, never as
   held-out results. Before a first scoring only aggregates of the previous
   binary are looked at, never per-case results.
5. **Frozen labels.** Evaluation applications and labels are frozen with
   SHA-256 checksums before the first scoring (`fixtures/eval/pNN/FROZEN.json`,
   [freezing rule](evaluation-datasets.md#freezing-rule)); scorers verify
   the checksums; labels are never edited, and corrections go into a
   separate `corrections` array with a date and a reason.
6. **Unsupported is reported, not scored away.** A positive the tool
   reports `UNSUPPORTED`, `BLOCKED` or `INCONCLUSIVE` is a false negative;
   entries labeled unsupported are kept out of the denominators and must
   be reported as unsupported, never claimed safe.
7. **Changes are generic.** A fix prompted by a set names no entry of it;
   a change prompted by a held-out project moves that project to the dev
   set.
8. **Divergence is reported.** A runtime run that violates the property
   along a different schedule is `diverged`, not a reproduction; failed and
   blocked runs are kept in the records.
9. **Compatibility is checked.** Static corpus outputs (four projects ×
   depths 3, 6, 12 × three path modes × json, sarif, text, trace: 144
   outputs) and the historical model-check scenarios are compared byte for
   byte with the previous binary; differences are classified.
10. **Privacy.** Private corpus projects appear only as aggregate counts
    and generic causes.

## Independence

**No independent human review has been performed.** The repository's
labels, reviews and evaluation applications were written by the
implementer or by AI sessions. The levels of separation that do exist:

| labels | who labeled | separation from the implementation |
| --- | --- | --- |
| first corpus review (`corpora/labels/`) | the implementer, from source code alone | none |
| blind corpus re-review (18 findings) | a separate automated session | blind to the earlier labels; not a human |
| `fixtures/eval/p11` | the implementer (first-party set); earlier hand-written scenarios' verdicts (held-out set) | none for the first-party set; held-out only for its first scoring |
| `fixtures/eval/p12` | the implementer, fresh application frozen before scoring | none |
| `fixtures/eval/p13`–`p15` | AI agents without access to `crates/` or the work in progress; they read earlier documents and fixtures | independent of the code, not of the project; fresh, not blind |
| `fixtures/eval/p16` | an AI agent that read no `crates/`, `runtime/` or design notes and ran no build or Midwrite command; it read the user-facing reference docs and the previous set's format | independent of the code, not blind to the documented subset |
| `corpora/eval/labels/` (static harness) | separate AI agents: auditors who never saw Midwrite's output, and finding labelers | separate jobs; not a human |

AI evaluation is not review. Statements in these documents that a result
is "verified" refer to the measurements above, never to human review.

## Static-finding review

Corpus findings are classified per finding (`corpora/labels/<project>.json`):
`true-positive` (the window exists and nothing in the code handles it),
`intended-risk` (the window exists but the code accepts it on purpose:
best-effort notifications, retried worker jobs, orderings a source comment
explains, seed and test code), `false-positive` (the claimed partial state
is not real), `unclear`. **Hazard precision** counts true positives and
intended risks as correct (the reported window exists); **bug precision**
counts true positives only. Large projects are sampled deterministically
(every k-th finding in report order), small ones reviewed completely;
intervals (95 % Wilson) are reported with the counts. `MW9001` is a
limitation, not a claim, and is not labeled. Recall needs audited known
positives and is reported only where they exist
([evaluation harness](../development/evaluation-harness.md)).

## Corpus

Two public projects at pinned revisions (saas-starter `6e33e58b`, midday
`51587319`) and two private projects (local copies with recorded local
changes) are analyzed statically only: `corpora/fetch.sh` clones the public
ones into `.corpora/` (ignored by git); manifests and per-project
configurations are under `corpora/`. No corpus project has ever been
executed. Procedure: [corpus evaluation](../development/corpus-evaluation.md).

## Runtime evidence procedure

Original-code evidence is produced by the evidence scripts
(`scripts/evidence.py` … `scripts/evidence.py`), which run
each scenario N times (5 on macOS and 3 in the Linux container for the
crash, in-flight and connection-loss sets; 3 for discovered scenarios) and
exit non-zero unless every scenario ran N times with the expected outcome
(a script whose set contains expected divergences says so). Reports are
checked against their [JSON Schemas](../reference/schemas/)
(`scripts/validate-schemas.py`, with deliberately corrupted copies that
must be rejected). Linux runs use a Debian 12 container on the same machine
(`scripts/linux-docker.sh`) with the source digest (`scripts/digest.py`)
compared to the macOS tree.

## Known weaknesses of the methodology

* Labeled sets are small (15 to 40 cases) and written by people or agents
  who knew the supported subset.
* First-party fixtures were written to contain the patterns they test;
  their reproductions are evidence about the patterns, not about other
  applications.
* Two PostgreSQL versions (17.11, 15.19), one architecture (arm64), one
  physical machine.
* No maintainer confirmation and no external application execution.

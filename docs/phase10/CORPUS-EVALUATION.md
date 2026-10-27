# Corpus evaluation (Phase 10)

Phase 10 did not change static analysis: `commitrift-core`,
`commitrift-analyzer` and `commitrift-corpus` have no source change since the
Phase 9 commit `81d27e4` (`git diff --stat 81d27e4 -- crates/commitrift-core/src
crates/commitrift-analyzer/src crates/commitrift-corpus/src` is empty; the only
change under them is the analyzer's fixture test, which now skips the Phase 10
applications as it skips those of Phases 7-9). The regression
criterion is therefore byte identity of every output, as in Phase 9.

## Corpus

Unchanged from Phase 9 (`docs/phase9/CORPUS-EVALUATION.md`): saas-starter and
midday (public, pinned clean checkouts under `.corpora/`), school-ledger and
unnamed-ai-project (private, local). No project code was executed.

## Diagnostic identity

`scripts/phase8-identity.sh reports/phase10/baseline/commitrift-phase9` with
`MODES="shared compact legacy"` against the final Phase 10 release binary:
`commitrift check` in json, sarif and text and `commitrift trace` in json, at
call depths 3, 6 and 12, in the three path modes, for every project.

**Result: all 144 outputs byte-identical** (`reports/phase10/final/corpus-identity.txt`;
the same sizes and SHA-256 prefixes as Phase 9's record). A first attempt
passed the old binary by a relative path, which the script resolves inside
each project directory, so the old side produced nothing and every line read
DIFFERENT; it is kept as `corpus-identity-relative-path-mistake.txt` and is
not a result. Consequently, against Phase 9: no change to diagnostics,
fingerprints, severity, confidence, JSON, SARIF, text output, traces or path
truncation. Finding counts are the Phase 8 numbers (unchanged since).

## Model-checking identity

`scripts/phase9-model-identity.py` compares, scenario by scenario, the reports
of the Phase 9 binary and the Phase 10 binary for the Phase 6–9 model
scenarios (verdicts, states, transitions, schedules, witnesses, assumptions,
diagnostics, text). The default search (sleep sets) is unchanged in Phase 10;
the differences that can appear are:

* scenarios run with `--dpor`: the classic search now handles every race, so
  its transition counts can grow (verdicts may only become *more* correct);
* `exploration.metrics` gains nothing for sleep-set runs (new counters are
  only serialized for source-DPOR);
* the terminal-state *sets* collected by tests (`collect_states`) are keyed
  modulo cross-channel effect order; this is not part of any report.

**Result: 56 of 56 scenarios identical**, JSON and text, with no exempt
field (`reports/phase10/final/model-identity.txt`; Phases 6–8 scenarios plus
`fixtures/phase9/scenarios` and `fixtures/phase9/model-scenarios`). The
Phase 9 and Phase 8 original-code regressions were also rerun with the final
binary (`reports/phase10/final/phase9-regression`, `phase8-regression`): 11
and 21 scenarios, 3 runs each, every run followed and matched; their 32
reports keep schema 3 and 4 and pass the report check (299 corruptions
rejected).

## External applications

No external application was executed in Phase 10, and no new external
finding is claimed. The saas-starter `inviteTeamMember` extraction is
exercised by the reduction benchmark (team-app workload, first-party
structural copy) only.

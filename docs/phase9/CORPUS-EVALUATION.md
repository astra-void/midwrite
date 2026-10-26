# Corpus evaluation (Phase 9)

Phase 9 did not change static analysis. `commitrift-core` and
`commitrift-analyzer` have no source change since the Phase 8 commit
(`4dc01af`; the only diff under them is a test file). The regression
criterion is therefore byte identity of every output, not "similar
findings".

## Corpus

| project | revision | kind | where |
| --- | --- | --- | --- |
| saas-starter | `6e33e58b1e553a41fe22e6b941a7229a002de361` | public, pinned | `.corpora/saas-starter` (clean checkout) |
| midday | `51587319f26a0ffaa9dfccab1920373cb65689b7` | public, pinned | `.corpora/midday` (clean checkout) |
| school-ledger | `4b97f342f68c42cf45d6ab8d939e0ec9368eede6` | private, local | maintainer's machine; the working tree has 2 uncommitted changes, analyzed as is by both binaries |
| unnamed-ai-project | `39fcfc7c8a0a0a56d9ade2faac71cefb63f41354` | private, local | same; 2 uncommitted changes |

Configurations: `corpora/configs/<project>.json`, unchanged since Phase 8.
No project code was executed: `commitrift check` and `trace` read source
only.

## Diagnostic identity

`scripts/phase8-identity.sh reports/phase9/baseline/commitrift-phase8`
(the saved Phase 8 release binary) against the final Phase 9 release
binary: `commitrift check` in json, sarif and text and `commitrift trace`
in json, at call depths 3, 6 and 12, in the three path modes (`shared`, the
default, `compact`, `legacy`), for every project.

**All 144 outputs are byte-identical** (`reports/phase9/final/corpus-identity.txt`). Consequently,
against Phase 8:

| | change |
| --- | --- |
| diagnostics (CP1001–CP9001, CP7001–CP7103) | none |
| fingerprints, severity, confidence | none |
| JSON, SARIF, text output | none (byte-identical; SHA-256 prefixes in the file) |
| traces, unknown calls, summary statistics | none (trace JSON byte-identical) |
| path truncation | none |

The run before the finalization fix (`reports/phase9/corpus-identity.txt`,
two path modes, 96 outputs) was identical as well; the final run adds the
`shared` mode and the final binary.

Finding counts are therefore the Phase 8 numbers recorded in
`docs/phase8/CORPUS-EVALUATION.md` (school-ledger 332 at depth 3; midday 282
at depth 3; saas-starter 23; unnamed-ai-project 17); they were not
recounted.

## Static analysis performance

Not re-measured: with identical code paths in core and analyzer, a timing
difference would measure the machine, not Phase 9. The Phase 8 numbers in
`docs/phase8/PERFORMANCE.md` stand.

## Model-checking corpus

The 40 model scenarios of Phases 6–8 (including the original saas-starter
`inviteTeamMember` extraction) give identical reports — verdicts, states,
transitions, schedules, witnesses, assumptions, diagnostics, text — with the
Phase 8 and the final Phase 9 binary; only `visitedBytes` differs, by design
(`reports/phase9/final/model-identity.txt`: 40 of 40 identical).

## Privacy

Committed reports for the private projects contain only the project's short
name, output sizes and SHA-256 prefixes of the outputs. No source path
inside those projects, no diagnostic message, no function name and no source
excerpt of theirs is committed. The absolute location of the two private
checkouts is the maintainer's local path already present in
`scripts/phase8-identity.sh` since Phase 8.

## Findings awaiting review

Unchanged from Phase 8: **no independent human review** of any corpus or
first-party finding has been performed. The 10 blind review packets for the
Phase 8 first-party recovery findings (`reports/phase8/review-packets/`) are
still unreviewed, and Phase 9 adds none: its findings concern a first-party
fixture written to contain the patterns (`fixtures/phase9/ledger-app`), not
an external application. No AI judgment is presented as a review.

# Phase 2 follow-up: the recommended next step

`FINAL-REPORT.md` recommended two things: recognize call-returned database
handles and methods of module-level service objects (the largest remaining
false-negative classes), and let teams declare best-effort external effects
instead of inferring intent. This document covers what was implemented,
how it was tested, and what it changed on the corpus. The numbers come from
`reports/phase2/corpus-next.json`, `reports/phase2/bench-followup*.tsv` and
the commands below.

## What was implemented

| Feature | Configuration | Tests |
| --- | --- | --- |
| **Handles returned by calls.** A `databaseReceivers` entry ending in `()` (`"getDb()"`, optionally with `importedFrom`) denotes the handle a call without arguments returns: direct chains (`getDb().transaction(...)`), `const` aliases (`const db = getDb()`) and arguments (`save(getDb(), id)`). Calls with arguments, unconfigured calls and `let` aliases are not matched. | `databaseReceivers` | `vulnerable/call_returned_handle.ts`; `phase2.rs`: `configured_call_paths_denote_the_returned_handle`, `call_paths_respect_imported_from_and_bind_as_arguments`; config validation test |
| **Object services.** `obj.m(...)` where `obj` is a `const` object literal of the same file, or an imported binding whose export is one, is summarized like a function call. An assignment or `delete` of one of its properties in its file disables it. Other receivers are not counted as imports. | none | `vulnerable/object_service.ts`, `projects/object_service` |
| **Best-effort effects.** `"bestEffort": true` on an `externalEffects` rule: under `recommended`, a CP1001/CP1005 whose effect that may not happen is best-effort becomes `info`, with a note. `"firstArgument"` narrows a rule to a string-literal first argument (`triggerJob("notification", …)`). | `externalEffects` | `phase2.rs`: `best_effort_effects_after_a_commit_are_informational`; config test |
| **Transaction results** (added after the evaluation below found two false positives): literals and objects returned by an inline transaction callback are facts about the `const` holding the transaction's result. | none | `safe/transaction_result_correlation.ts`; `phase2.rs`: `transaction_results_correlate_with_later_tests` |

Class methods and `this.method()` calls remain unsupported. In midday, 11 of
the 158 context CP9001 calls are `this.method()`; following them soundly
needs a rule for subclass overrides, which was not designed.

## Effect with the original corpus configurations

Same projects, same (pre-review) configurations as
`reports/phase2/corpus-phase2.json`:

```bash
cargo run --release -p crashproof-corpus -- run --manifest corpora/manifest.json \
  --manifest corpora/manifest.local.json --output after.json
cargo run --release -p crashproof-corpus -- compare reports/phase2/corpus-phase2.json after.json
# 0 added, 0 removed, 0 changed
```

No finding changed. Object services need no configuration and linked 35
more calls in midday (its `apiKeyCache`/`teamCache` objects, 1262 → 1297),
but those methods perform no recognized operation, so no summary was used.
The other features need configuration.

Performance with these configurations (10 runs, `bench-followup.tsv`):
midday 0.1634 s median (Phase 2: 0.1563 s), school-ledger 0.1202 s (0.1177 s),
saas-starter 0.0088 s (0.0076 s), unnamed-ai-project 0.0103 s (0.0099 s). The
difference is within 5% for the two larger projects, about the run-to-run
spread seen before. RSS is unchanged (74.5 and 75.4 MiB).

## Effect with configurations written after the review

`corpora/manifest.next.json` and `corpora/manifest.local.next.json` use
variants written **after** the Phase 2 review, with the review's knowledge.
This shows what the features do. It is not an independent measurement of
precision.

* `configs/midday.next.json` adds best-effort rules for notification jobs
  (`triggerJob("notification", …)`, `tasks.trigger("notification", …)`,
  `notificationsQueue.add`) and transactional emails (`resend.emails.send`,
  `resend.batch.send`).
* `configs/school-ledger.next.json` adds `"getDb()"` from `@/server/db` and
  excludes `tests/**`, `scripts/**`, `e2e/**`.
  `configs/school-ledger.getdb.json` adds only `"getDb()"`.

### midday: best-effort declarations

Warnings at the default policy: **49 → 36**. Of the 13 findings that became
`info`, the labels classify 12 as intended risk and 1 as unclear; **no
reviewed true positive changed severity**. Finding identities are unchanged
(same fingerprints), so the effect could be checked against the labels
directly.

### school-ledger: `getDb()` handles

| Configuration | Functions with effects | Calls summarized | Truncated | Application code (`src/`) | Tests and scripts |
| --- | --- | --- | --- | --- | --- |
| original | 526 | 1310 | 12 | CP9001 ×5 | CP1004 ×123, CP9001 ×189 |
| + `getDb()` | 1181 | 4225 | 59 | CP9001 ×10 | CP1004 ×988, CP9001 ×604 |
| + `getDb()` + `exclude` | 347 (325 files) | — | 1 | CP9001 ×10 | — |

The application's database code is now visible to the analysis (more than
twice as many functions with effects). The first run with `getDb()`
reported two CP1004 in `src/` (`point-reports.ts:183`,
`correction-requests.ts:424`). Both were **false positives**: the
transaction callback returns `"forbidden"` before writing, and the denial
audit runs only `if (result === "forbidden")`. That finding led to the
transaction-result facts above; with them, `src/` has no hazard finding.
With test files analyzed, `getDb()` makes test setup visible as well (988
CP1004, 59 truncated test functions), so `exclude` matters even more for
this project. With `exclude`, the scan takes 0.055 s and 37 MiB
(`bench-followup-next.tsv`).

No true positive was found in school-ledger's application code. That
matches the Phase 2 review, which found no application-level CP1004 there:
its services use transactions and pass `tx` to helpers consistently.

## What remains

* Class methods and `this.method()`.
* Return-value correlation for helpers (`const ok = await check(); if (ok)`)
  and for non-literal values.
* The remaining non-bug findings in midday are mostly job-retry-driven
  (worker steps) and documented orderings; neither can be stated in
  configuration today.
* A second, independent reviewer for the labels.

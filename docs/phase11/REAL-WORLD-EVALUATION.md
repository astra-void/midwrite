# Real-world evaluation

Raw: `reports/phase11/evidence/corpus-*.json`, `SUMMARY.md`.

## Corpus and access

| project | revision | access | analyzed as |
| --- | --- | --- | --- |
| saas-starter | `6e33e58b` | public, pinned clean checkout (`.corpora/`) | the checkout |
| midday | `51587319` | public, pinned clean checkout | the checkout |
| school-ledger | `4b97f342` | private, local | `git archive` of the pinned revision into a scratch directory (the working tree has since moved to `d1fdf38`; it was not touched) |
| unnamed-ai-project | `39fcfc7c` | private, local | `git archive` of the pinned revision |

No corpus project was executed: discovery and bounded model checking only.
There is no contracts file for any of them; none was written (it would be
an assumption about their behavior). Private projects: aggregate counts
only.

## Discovery

| project | modules | entry candidates | not reaching a db | extracted | effectful | unknown ops | tables modeled / refused | candidates | counterexamples | wall | peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: |
| saas-starter | 41 | 44 | 7 | 37 | 8 | 274 | 5 / 0 | 2 | 1 | 0.07 s | 10.8 MiB |
| midday | 2 022 | 2 864 | 1 696 | 1 168 | 0 | 15 382 | 8 / 42 | 0 | 0 | 28.6 s | 72.3 MiB |
| school-ledger | 461 | 880 | 334 | 546 | 0 | 9 029 | 9 / 21 | 0 | 0 | 3.5 s | 49.3 MiB |
| unnamed-ai-project | 104 | 74 | 41 | 33 | 1 | 1 009 | 7 / 2 | 0 | 0 | 0.08 s | 11.7 MiB |

### saas-starter

* `app/(login)/actions.ts#inviteTeamMember/arg[1]` — **check-then-act on
  `invitations`** (email, team from the user's membership, status
  `pending`), discovered without any scenario or contract: the wrapper's
  guards became a synthesized precondition, the user, team and membership
  rows were synthesized through the join, and the model found the
  duplicate pending invitation. Property origin `HEURISTIC_HYPOTHESIS`; not
  executed; not reviewed. This is the race Phase 7 checked from a
  hand-written scenario.
* A second check-then-act (`signUp`, `users.email`) is protected by
  `UNIQUE(email)` (`SCHEMA_ENFORCED`); it is **INCONCLUSIVE**: the generated
  scenario does not load, because a local (`invitation`) is assigned only by
  an operation the extractor did not understand, and the loader refuses a
  read of a never-assigned local. It is not reported safe.

### midday, school-ledger, unnamed-ai-project

No candidate. The dominant blocker is the schema: 42, 21 and 2 tables are
refused by the loader because their primary key is not a modeled column —
`uuid(...).defaultRandom()` / `$defaultFn(...)` keys, which Phase 8's
loader refuses rather than invent values for. With the tables unbound,
nearly every database operation is unknown, so no entry has an effect
discovery could pair. These are reported, not hidden: every refused table
and every unknown operation is in the report.

## Unknown operations by category

| category | saas-starter | midday | school-ledger | unnamed-ai |
| --- | ---: | ---: | ---: | ---: |
| unresolved method call | 123 | 5 433 | 4 149 | 625 |
| call into an unanalyzed package | 55 | 4 136 | 521 | 67 |
| unsupported expression | 47 | 1 631 | 1 455 | 139 |
| unresolved import | 0 | 1 132 | 343 | 0 |
| unsupported binding pattern | 4 | 706 | 269 | 16 |
| unresolved identifier | 0 | 659 | 517 | 7 |
| unresolved call | 4 | 538 | 328 | 0 |
| unsupported transaction shape | 0 | 318 | 104 | 0 |
| loop over a collection of unknown length | 0 | 133 | 305 | 54 |
| re-export not followed | 0 | 82 | 294 | 0 |
| inline depth limit | 0 | 25 | 321 | 20 |
| table / column not modeled (schema) | 12 | 0 | 0 | 49 |
| unsupported query shape | 17 | 1 | 0 | 2 |
| other | 12 | 588 | 423 | 30 |

## Unsupported coverage, as engineering priorities

1. **Generated primary keys** (`defaultRandom`, `$defaultFn`): unblocks
   midday, school-ledger and unnamed-ai-project's tables.
2. **Method calls and package calls** (repository objects, SDK clients):
   the largest category everywhere; most need declared externals or object
   resolution.
3. Re-exports and imports through barrels (midday, school-ledger).
4. Transaction shapes beyond an inline `db.transaction(async (tx) => …)`.

## Review status

**No independent human review was performed.** No finding about a corpus
project is claimed as a defect: the one counterexample (saas-starter) is
against a heuristic hypothesis, model-only, never executed. A review
packet would contain, per candidate: the source excerpt where
redistribution is permitted (saas-starter is MIT), the candidate
explanation from the report, the property origin and rationale, the
assumptions, the model counterexample, the operations it exercised, the
unknown operations of the entry, and the proposed verification
(`--emit-scenarios` + an authorized harness). The JSON report holds all of
it; no packet was sent to anyone.

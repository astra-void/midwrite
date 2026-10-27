# Precision and recall

Raw: `reports/phase11/evidence/SUMMARY.md`, `commerce-app.json`,
`heldout-*.json`; first scoring of the held-out set:
`reports/phase11/eval/heldout-first-scoring.md`. Scoring code:
`commitrift-discover/src/eval.rs` (`commitrift discover --labels FILE`).

## What is measured, separately

| measurement | definition |
| --- | --- |
| static hazard prediction | the entry has a candidate of its own (all actors are this entry), not blocked, with no recognized protection |
| model prediction | the entry has a candidate of its own whose deciding scenario has a validated model counterexample |
| class agreement | for a model-predicted positive: a candidate of the labeled class has the counterexample |
| unsupported handling | an entry labeled `unsupported` is reported with unknown operations (or a failed extraction) and none of its candidates claims a bounded safe result |
| candidate → model | candidates with a satisfiable scenario that got a model result |
| model → runtime | model counterexamples with a harness mapping that reproduced on the original code |

Precision and recall are computed over `positive`/`negative` labels only.
No single accuracy number is reported. Property interpretation (whether the
application *intends* the property) is not measured: no independent person
reviewed the findings (`REAL-WORLD-EVALUATION.md`).

## 1. First-party benchmark (`fixtures/phase11/labels.json`)

21 labeled entries of `fixtures/phase11/commerce-app`: 8 positive, 10
negative, 3 unsupported. Labels were written with the fixture **before the
engine existed**, by the same author as the engine: they are not
independent, and the fixture was designed with the detectors in mind. No
label was changed (`corrections` is empty).

| | TP | FP | FN | TN | precision | recall | F1 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| static hazard prediction | 8 | 2 | 0 | 8 | 0.800 | 1.000 | 0.889 |
| **model prediction** | **8** | **1** | **0** | **9** | **0.889** | **1.000** | **0.941** |

* class agreement: 7 agree, 2 differ — `claimTask` (labeled
  `check-then-act`, found as `status-transition`, the finer class the
  labels did not have) and the false positive;
* unsupported: 3 of 3 reported (`db.execute` with a dynamic string, an
  undeclared client call, `request.json()`), 0 claimed safe;
* candidate → model: 20 of 20 satisfiable candidates model-checked; 2
  pairing candidates were unsupported (their partner `reserveItemAtomic`
  returns a mutation's rows from a helper — "an array of rows used as a
  value"), so `reserveItemAtomic` is a true negative only because it has no
  candidate of its own.
* **False positive** — `checkoutIdempotent`: two concurrent checkouts of
  one order with the same request id; the provider deduplicates the charge
  (declared), but both requests insert a `charges` row. The violated
  property is the heuristic "at most one `charges` row per transition". The
  label's property (one effective charge) holds. It is a real
  database/external-effect inconsistency the label did not consider, and
  it is reported as a counterexample against a `HEURISTIC_HYPOTHESIS`, not
  as a defect. Counted as FP; the label was not changed.
* Static FP (2): `checkoutIdempotent` (same) and `processRefundOnce`, whose
  status-transition candidate has no recognized protection although the
  model finds no violation (the claim row protects the payout).

## 2. Held-out labels (Phase 7/8 applications)

`fixtures/phase11/heldout/labels.json`: 12 entries of `fixtures/phase8/shop-app`
and `fixtures/phase7/team-app` labeled from the verdicts of their 23
hand-authored Phase 7/8 scenarios (written before this work, by the same
project). Contracts transcribe only what those scenarios declared
(externals, deduplication, retries, delivery, the retry helper; no
invariant).

**First scoring** (before any change prompted by it):

| app | TP | FP | FN | TN | precision | recall |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| shop-app | 4 | 2 | 2 | 2 | 0.667 | 0.667 |
| team-app | 0 | 0 | 2 | 0 | — | 0.000 |
| **both** | **4** | **2** | **4** | **2** | **0.667** | **0.500** |

The misses and false alarms were then analyzed and fixed: the
`rows.length` idiom, joined tables, join-aware seeding, read-derived keys,
state tests in the read's WHERE clause, serial outcomes that account for
failed requests (the two FPs: REPEATABLE READ with retries is serializable),
transaction retries using the retry bound, effects inside retried
transactions, the `distinct-request-key` variant. **After** (no longer a
held-out result):

| app | TP | FP | FN | TN | precision | recall |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| shop-app | 5 | 0 | 1 | 4 | 1.000 | 0.833 |
| team-app | 2 | 0 | 0 | 0 | 1.000 | 1.000 |

Remaining FN: `shipOrderDualWrite` (commit, then publish; a crash loses the
event). Its property — "a shipped order's event is not lost" — is a
recovery requirement no detector synthesizes: inventing it would be
inventing a requirement. Declared as a contract invariant it would be
checkable.

## False-positive causes (all benchmarks)

1. Heuristic properties broader than the labeled property
   (`checkoutIdempotent`).
2. Before the fix: properties that ignored legitimate failures
   (serialization failures under REPEATABLE READ).

## False-negative causes

1. Requirements that are not synthesized (eventual delivery /
   not-lost events) — by design.
2. Before the fixes: extraction idioms not traced (length), tables only
   joined, values read from the database used as keys.

## Unsupported-pattern breakdown

See `REAL-WORLD-EVALUATION.md` for the corpus categories. On the
first-party application: 15 unknown operations in 8 entries (unresolved
method calls of undeclared clients 8, unresolved identifiers 3, raw SQL 1,
other 3).

## Model → runtime

10 generated counterexample scenarios of the commerce application have a
harness mapping; **10 of 10 reproduced on the original code in 3 of 3 runs**
(9 `original-code-reproduced`, 1 `original-code-crash-reproduced`).

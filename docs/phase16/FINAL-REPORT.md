# CommitRift Phase 16 final report

Real-world modelability, TypeScript semantic coverage and Closed Alpha
hardening. Base commit `1efa383` (the Phase 15 final tree, verified by
source digest, [BASELINE.md](BASELINE.md)). **Nothing was committed,
pushed, published or released.**

**No independent human review was performed.** All implementation,
labels and checks are the work of AI agents. An AI agent checking another
AI agent's results is not independent human validation.

## Summary

* Seven tracks were implemented in parallel against nine written interface
  agreements ([ARCHITECTURE.md](ARCHITECTURE.md)) and integrated into one
  tree.
* **Correctness:**
  * 0 disagreements with PostgreSQL in every new lockstep matrix
    (NULLS NOT DISTINCT: 1 542 schedules; multi-row locks: 2 124) and in
    every replayable discovered counterexample (49 of 54; the other 5 are
    replica-only);
  * 0 mismatches with the pinned zod runtimes (1 220 comparisons);
  * 0 POR discrepancies;
  * 56/56 model reports and 144/144 static outputs byte-identical to
    Phase 15.
* **Evaluation:** on the fresh frozen set, the first score is precision
  1.000 and recall 0.333 (Phase 15 binary: 0.056); the post-feedback score
  is unchanged.
* **Regressions:** historical sets improved on Phase 14 and 15. One new
  Phase 12 false positive is explained, not hidden.
* **Real world:** one previously blocked midday candidate is now
  model-checkable. school-ledger stays at 0, with its blocker one
  validation step later.
* **Linux:** every gate passes on the same source digest, including
  clippy with rustc 1.99.1 (§8.5).

## 8.1 Implementation

| area | what is supported now | document |
| --- | --- | --- |
| zod constructs | `object` (strip), `strictObject`/`.strict()`, `looseObject`/`.passthrough()`, `.optional()`/`.nullable()`/`.nullish()`/`.default(literal)`, `string` with length, case, trim and affix checks, `uuid`, `email`, an ECMAScript `regex` subset, `number`/`int` with bounds, `boolean`, `enum`, `literal`, `array` with length checks; `parse` and `safeParse`; zod 3 and zod 4 semantics chosen from the project's installed or declared version; refinements, transforms, unions, `extend`/`pick`/`omit` and other formats refused by name |
| structured objects | objects are `Value::Row` with absent ≠ `undefined` ≠ `null`; builtins `HasOwn`, `OmitKeys`, `ObjectAssign`, `DefinedEntries`, `ZodCheck`, `ZodData`; input synthesis conforms arguments to the schema (`zodfn::conform`) and explores `absent:`/`undefined:`/`null:` worlds | same |
| loop semantics | `for`/`while`/`do…while` with an established counter bound unroll into `Let`/`JumpIf`/`Jump` (never merged); classifications COMPLETED, BOUNDED_UNROLLING, TRUNCATED (beyond 8), UNKNOWN, UNSUPPORTED (`for…in`, `for await`, labeled jumps); `for…of` over known arrays | [LOOP-SEMANTICS.md](LOOP-SEMANTICS.md) |
| retry handling | a retry loop around a write or transaction re-executes after a lost acknowledgment or a caught SQLSTATE; a `catch` reads `code` only for verified origins (drizzle-orm 0.45's `DrizzleQueryError`: `cause.code` holds the SQLSTATE, `code` is undefined) | same |
| request and response | `await request.json()` in a route handler is the entry's synthesized argument; `Response.json`, `NextResponse.json` and `new Response` are its return value `{status, body}` | [FRAMEWORK-REQUESTS.md](FRAMEWORK-REQUESTS.md) |
| predicate resolution | predicates held in `const` locals, aliases, operand capture at declaration, conditional predicates, `return <pred>` helpers (local and imported); reassigned `let`s and clock readings refused | [DRIZZLE-PREDICATES.md](DRIZZLE-PREDICATES.md) |
| multi-row locking | multi-row `SELECT` (`many: ManyRows{limit, order_by}`) with `FOR UPDATE/NO KEY UPDATE/SHARE/KEY SHARE`, `NOWAIT`, `SKIP LOCKED`, `LIMIT n`; rows locked one at a time; NOWAIT releases at the error | [MULTI-ROW-LOCKS.md](MULTI-ROW-LOCKS.md) |
| nullable uniqueness | `UNIQUE … NULLS NOT DISTINCT` (`.nullsNotDistinct()` in Drizzle) extracted, enforced at statement time with waits/23505, in `ON CONFLICT` arbiters and POR footprints; the Phase 15 INCONCLUSIVE rule narrowed to rules with unmodeled columns | [NULLS-NOT-DISTINCT.md](NULLS-NOT-DISTINCT.md) |
| generated-ID invariants | `generatedBy`, `sameGenerated`, `distinctGenerated`, `forEachActor`, `forEachRow`, terms `returned`/`field`/`argument`/`countMatching`/`rowMatching`; `let t = generate "…"` in the DSL; heuristic serial outcomes stated over generated identity without fabricating concrete IDs; diagnoses of final values; contract templates | [GENERATED-ID-INVARIANTS.md](GENERATED-ID-INVARIANTS.md) |
| report schema and CLI | `discovery-report.v1` and `counterexample.v1` JSON Schemas with positive and negative tests; grouped unsupported messages, declaration skeletons, reproduction commands, loop bounds under `--explain-coverage` | [QUALITY-GATES.md](QUALITY-GATES.md) |

## 8.2 Correctness

| | result |
| --- | --- |
| test counts | 843 passed, 0 failed, 3 ignored on macOS and on Linux (Phase 15 baseline: 751 / 0 / 3) |
| PostgreSQL differential | NULLS NOT DISTINCT 1 542 schedules, multi-row locks 2 124, 0 disagreements; discovered counterexamples 49/54 confirmed (264 outcomes, 0 disagreements; 5 replica-only); approximation measured: lock order without `ORDER BY` (24 of 51 differ) ([POSTGRESQL-DIFFERENTIAL.md](POSTGRESQL-DIFFERENTIAL.md)) |
| zod differential | 1 220 comparisons, 943 agreements, 277 refusals, **0 mismatches**; 95/95 synthesized inputs accepted ([MODEL-CORRECTNESS.md](MODEL-CORRECTNESS.md#2-zod-model-vs-the-pinned-runtimes)) |
| POR agreement | 197/197 emitted scenarios × 4 searches; locks 180 + 599 comparisons; NULLS NOT DISTINCT 90 models × 6 configurations; 0 discrepancies; two mutation checks fail as expected ([MODEL-CORRECTNESS.md](MODEL-CORRECTNESS.md#1-partial-order-reduction-on-the-new-semantics)) |
| original-code reproduction | `placeOrder` and its route handler: duplicate order after a lost COMMIT acknowledgment, reproduced 3/3; Phase 15 worlds-app 3/3 each; safe counterparts `recordPayment` (recovery reproduced 2/2), `withdraw` (invariant held), `createLabel` (invariant held 3/3) |
| known model approximations | multi-row lock order without `ORDER BY`; `generatedBy` per actor and generator, not per occurrence; one replica, model-only; generated identity `not-evaluated` on original code ([LIMITATIONS.md](LIMITATIONS.md)) |
| unsupported constructs | always reported as UNSUPPORTED with the first construct and its location ([LIMITATIONS.md](LIMITATIONS.md#not-modeled-always-reported-never-treated-as-safe)) |
| regression | historical sets below (§8.4); compatibility 56/56 and 144/144 identical |

## 8.3 Real-world coverage

| metric | Phase 15 | Phase 16 |
| --- | ---: | --- |
| midday effectful entries | 81 | 86 |
| midday candidates | 4 | 4 |
| midday model-checkable | 0 | **1** |
| school-ledger effectful entries | 70 | 70 |
| school-ledger candidates | 9 | 9 |
| school-ledger model-checkable | 0 | 0 |
| saas-starter model-checkable | 1 | 1 |

Phase 15 values are the Phase 15 binary re-measured today on the same
inputs; they equal the brief's baseline. Every remaining blocker is listed
in [REAL-WORLD-COVERAGE.md](REAL-WORLD-COVERAGE.md#every-remaining-corpus-blocker):

* school-ledger, 7 candidates: zod `.superRefine()` (an arbitrary
  callback; accepting it would bypass validation);
* school-ledger, 2 candidates: a method call on an unresolved type;
* midday insight generation: `?.` on a call result and a `Promise.all`
  with a used result;
* midday, 2 write skews: need a declared invariant. None was invented;
  templates are provided.

The portal-settings candidate is checked against a **heuristic** property
and holds within bounds. That is not a finding and not a proof.

Cost: midday 1.23 s / 149 MiB → 1.77 s / 230 MiB for a larger workload
(+26 % effectful-or-read-only entries; replica reads 99 → 225).
Normalized, that is 8.5 → 10.4 ms per entry ([PERFORMANCE.md](PERFORMANCE.md)).

## 8.4 Evaluation

| | TP | FP | FN | TN | precision | recall |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Phase 16 set, Phase 15 binary | 1 | 0 | 17 | 19 | 1.000 | 0.056 |
| **Phase 16 set, first scoring** | **6** | **0** | **12** | **19** | **1.000** | **0.333** |
| Phase 16 set, post-feedback (final) | 6 | 0 | 12 | 19 | 1.000 | 0.333 |
| Phase 12 set (regression) | 6 | 1 | 1 | 5 | 0.857 | 0.857 |
| Phase 13 set (regression) | 8 | 0 | 7 | 11 | 1.000 | 0.533 |
| Phase 14 set (regression) | 7 | 0 | 11 | 18 | 1.000 | 0.389 |
| Phase 15 set (regression) | 10 | 0 | 3 | 14 | 1.000 | 0.769 |

* **Unsupported cases:** 3 labeled-unsupported cases, outside every
  denominator, all reported as unsupported (0 claimed safe). On a positive
  case, an UNSUPPORTED, BLOCKED or INCONCLUSIVE result counts as FN.
* **Model-checkable rate (Phase 16 set):** 12 of 26 candidates.
* **Counterexample validity:** 6 of 6 validated in the model; on
  PostgreSQL, see §8.2.
* **Phase 12 false positive:** `transferCredits` deadlocks (40P01,
  PostgreSQL-confirmed) against a heuristic "no invocation fails", outside
  the label's environment. Kept and counted.
* **First score:** recorded once and preserved
  (`reports/phase16/eval/first-scoring.json` with binary sha256 and source
  digest).
* **Human review:** none. A review packet is in
  [PRECISION-RECALL.md](PRECISION-RECALL.md#review-packet).

## 8.5 Cross-platform

Same source digest on both platforms (`93520c3f…`, 1 038 files):

| | macOS arm64 | Linux aarch64 (Docker, Debian 12) |
| --- | --- | --- |
| Rust | 1.98.1 | 1.99.1 |
| PostgreSQL | 17.11 | 15.19 |
| fmt / clippy `-D warnings` | exit 0 / exit 0 | exit 0 / **exit 0** |
| full suite, replay + schema + zod required | **843 passed, 0 failed, 3 ignored** (83 targets) | **843 passed, 0 failed, 3 ignored** (83 targets) |
| release build | exit 0 | exit 0 |
| report-schema validation | 0 errors (built-in and `jsonschema`) | 0 errors |
| Phase 16 evidence | 49/54 confirmed, 0 disagreements, reproductions 3/3 | identical |

Remaining platform-specific problems: none. The zod differential's
default package paths are machine-specific (set by environment variable;
the Linux script installs the pinned packages). Details:
[QUALITY-GATES.md](QUALITY-GATES.md#10-final-gate-results).

## 8.6 Developer alpha

[ALPHA-READINESS.md](ALPHA-READINESS.md):

* **Walkthrough:** `scripts/phase16-alpha-check.sh` runs every guide
  command, and every step behaves as described. The run found one guide
  promise the CLI did not keep; fixed.
* **Quickstart:** every example re-run. One wrong exit-status sentence
  corrected.
* **Package:** the guide, a feedback template and a bug-report template
  are prepared, **not distributed**.
* **Release blockers:** no human review; low recall on realistic code; an
  approximate lock order without `ORDER BY`; no packaged binaries
  (intended).

## Mandatory vertical slices

| slice | result |
| --- | --- |
| 1. school-ledger validation path | guard → synthesized valid actor (2 stated preconditions) → **zod strict object passes** (modeled and differentially checked) → blocked at the next construct, `.superRefine()` on an array, before any database effect. The exact next blocker is identified; the private project was not executed |
| 2. midday generated-ID property | no suitable declaration exists in midday, and none was invented. The first-party fixture `fixtures/phase16/genid` demonstrates the declared property (`share-token-told-is-stored`, MODEL_COUNTEREXAMPLE; atomic counterpart NO_VIOLATION). On midday itself, the heuristic serial-outcome property over generated identity is now expressible, and the candidate is model-checked in 3 worlds |
| 3. retry loop | `for` retry loop → ambiguous COMMIT → retry decision → duplicate `orders` row → model counterexample → **reproduced on the original code, 3/3**; idempotent counterpart `recordPayment` NO_VIOLATION_WITHIN_BOUNDS with recovery reproduced on the original code |
| 4. multi-row locking | adversarial order (two single-row locks in argument order → 40P01 counterexample) and safe order (`.orderBy(id)` multi-row lock → NO_VIOLATION); model and PostgreSQL agree on 2 124 schedules |
| 5. NULLS NOT DISTINCT | `.nullsNotDistinct()` extracted → concurrent NULL inserts conflict (23505) exactly as PostgreSQL (1 542 schedules) → the Phase 15 safe counterpart `createLabel` moves from INCONCLUSIVE to NO_VIOLATION_WITHIN_BOUNDS |
| 6. developer Closed Alpha | all eight steps validated by script |

## Checkpoints

| checkpoint | state |
| --- | --- |
| 0 baseline | digest equal to Phase 15's; 751/0/3; carried-in failures recorded |
| 1 shared design | [ARCHITECTURE.md](ARCHITECTURE.md): ownership and nine interface agreements |
| 2 parallel implementation | seven tracks with tests and documents |
| 3 first integration | merged; two schema/label seams and two false-positive regressions fixed |
| 4 differential verification | §8.2 |
| 5 real-world coverage | §8.3 |
| 6 frozen evaluation | set frozen and scored before feedback; first score preserved |
| 7 full regression | historical sets, identity gates, full suite |
| 8 cross-platform | §8.5 |
| 9 alpha readiness | walkthrough and quickstart validated |

## Definition of Done

| # | item | status |
| ---: | --- | --- |
| 1 | Phase 1–15 regressions remain valid | **yes**: full suite 843/0/3 on both platforms; 56/56 and 144/144 identity; historical sets equal or better except one explained Phase 12 FP |
| 2 | Linux clippy failure resolved without global suppression | **yes**: clippy `-D warnings` exit 0 on Linux with rustc 1.99.1 (the version that failed); no `allow` added |
| 3 | discovery-report schema implemented and validated | **yes**: positive tests on every Phase 16 fixture report and the committed historical discovery reports, plus the alpha run and (locally) the midday and school-ledger reports; 36 negative cases; real `jsonschema` and built-in validator; CI step added |
| 4 | zod strict-object semantics match the pinned runtime | **yes**: 0 mismatches in 1 220 comparisons |
| 5 | zod constraints integrate with synthesized actor inputs | **yes**: 95/95 conformed inputs accepted; school-ledger's synthesized actors pass `strictObject` |
| 6 | school-ledger's nine candidates re-evaluated | **yes**: per candidate, aggregated ([REAL-WORLD-COVERAGE.md](REAL-WORLD-COVERAGE.md#school-ledger-the-nine-candidates-aggregate)) |
| 7 | bounded retry loops modeled faithfully | **yes**: reproduced on original code; SQLSTATE handling verified against the driver stack |
| 8 | request-body handling for a documented subset | **yes** |
| 9 | hoisted Drizzle predicates resolve correctly | **yes**: including operand capture; P15-E06 found |
| 10 | multi-row locking matches PostgreSQL | **yes** for the modeled forms (0 disagreements); without `ORDER BY` stated as approximate (measured) |
| 11 | NULLS NOT DISTINCT enforced correctly | **yes** (1 542 schedules, 0 disagreements) |
| 12 | historical false-positive safe counterpart re-evaluated | **yes**: `createLabel` NO_VIOLATION_WITHIN_BOUNDS, invariant held on original code 3/3 |
| 13 | generated-ID properties without fabricated IDs | **yes** |
| 14 | midday's four candidates re-evaluated | **yes**: stage table per candidate |
| 15 | a new first-party source-level counterexample reproduced on PostgreSQL | **yes**: `placeOrder` / route `POST`, 3/3 |
| 16 | a safe counterpart verified within explicit bounds | **yes**: `recordPayment`, `withdraw`, `createLabel`, `ensureShareTokenAtomic`, `.orderBy` lock |
| 17 | POR tests cover the new database semantics | **yes** (§8.2) |
| 18 | a new frozen evaluation scored before feedback | **yes** |
| 19 | historical scores preserved | **yes**: no earlier report rewritten; re-runs written to new files |
| 20 | candidate reports explicit about unsupported operations and input bounds | **yes**: first blocker with location; `Input domain exhaustive: no`; loop bounds |
| 21 | macOS and Linux gates on matching source | **yes**: same digest; all gates pass on both (§8.5) |
| 22 | CLI quickstart examples run | **yes** (one sentence corrected) |
| 23 | Closed Alpha review package prepared | **yes**, not distributed |
| 24 | documentation and report links valid | **yes**: `scripts/check-doc-links.py`, 0 broken (§8.5) |
| 25 | final report with reproducible evidence and unresolved limitations | this document; [LIMITATIONS.md](LIMITATIONS.md) |

## Reproducing the evidence

```bash
cargo build --release
COMMITRIFT_REQUIRE_REPLAY=1 COMMITRIFT_REQUIRE_SCHEMA=1 COMMITRIFT_REQUIRE_ZOD=1 cargo test --workspace --no-fail-fast
python3 scripts/phase16-eval.py OUT.json                      # frozen set (verifies FROZEN.json)
python3 scripts/phase16-evidence.py OUT_DIR --repeat 3        # POR, PostgreSQL lockstep, original code
python3 scripts/phase16-corpus.py OUT_DIR --old PHASE15_BIN   # corpus (needs .corpora/)
python3 scripts/phase16-perf.py OUT_DIR --old PHASE15_BIN
scripts/phase16-alpha-check.sh OUT_DIR
scripts/phase16-linux-docker.sh OUT_DIR                       # Linux gates in a container
python3 scripts/check-doc-links.py
```

Raw outputs are under `reports/phase16/`.

## Unresolved

* **Recall on realistic code is low.** The next steps, by measured
  frequency:
  * JavaScript standard-library calls;
  * status transitions keyed by the id of a row read first (detection);
  * zod refinements;
  * optional chains on call results;
  * `Promise.all` with a used result.
* **No independent human review.**
* **Multi-row lock order without `ORDER BY` is approximate.** Read-replica
  counterexamples remain model-only.
* **Test defaults:** the zod differential's default package paths are
  machine-specific (overridable; the Linux script sets them).

# Phase 16 model correctness

(Track H's H2–H3 and the brief's §8.2, model part. PostgreSQL comparisons
are in [POSTGRESQL-DIFFERENTIAL.md](POSTGRESQL-DIFFERENTIAL.md).)

## 1. Partial-order reduction on the new semantics

Every reduction is compared with the unreduced search (`--no-por`): same
verdict, same completeness, and (in the per-track tests) the same terminal
states and counterexample existence.

| source | models | comparisons | discrepancies |
| --- | ---: | ---: | ---: |
| evidence run: every scenario emitted by discovery on ten applications, including the loop/retry, zod, lock, NULLS NOT DISTINCT and generated-identifier applications (`reports/phase16/evidence`) | 197 | 197 × 4 algorithms (unreduced, sleep sets, classic DPOR, source-set DPOR) | **0** |
| multi-row locks: the 36 differential models (`phase16_locks.rs`, `reductions`) | 36 | 180 | **0** |
| multi-row locks: 120 generated scenarios (2–3 transactions, 4 rows and a child table, every lock strength and option, random isolation) | 120 | 599 | **0** |
| NULLS NOT DISTINCT: all lockstep models (`phase16_nulls.rs`), sleep sets / classic DPOR / source-set DPOR with and without the state cache, plus `audit_independence` | 90 | 90 × 6 | **0** |

**The tests can fail.** Two mutation checks show that these comparisons
catch missing dependencies:

* With every `LockCheck`/`LockChange` access deleted from the footprints,
  sleep sets and source-set DPOR lose terminal states or counterexamples
  on 4 of the 12 lock workloads.
* With `key_overlap` reverted to skip NULL keys under NULLS NOT DISTINCT,
  `audit_independence` reports a violation on the first NND workload.

**New footprints:**
* Multi-row locks and reads: per row, as single-row locks.
* NULLS NOT DISTINCT keys: two NULL keys conflict.
* Caught-error inspection and loop counters: actor-local, no footprint.
* Generated values: actor-local, as in Phase 14.

**State counts of the evidence run** (total over the 197 scenarios):

| search | states | relative to unreduced |
| --- | ---: | ---: |
| unreduced | 4 015 | 1.00 |
| sleep sets | 4 015 | 1.00 |
| source-set DPOR | 3 543 | 0.88 |
| classic DPOR | 35 391 | 8.81 |

Sleep sets prune transitions, not states, so their state count equals the
unreduced one. Classic DPOR's count includes the states it revisits when a
backtrack set grows, so on these small two-actor models it explores more.
Source-set DPOR is the only one with fewer states. This matches Phases
13–15. Sleep sets remain the default for these model sizes.

## 2. zod: model vs the pinned runtimes

`crates/commitrift-analyzer/tests/phase16_zod.rs`
([ZOD-VALIDATION.md](ZOD-VALIDATION.md#differential-verification-against-the-real-zod)):
58 schemas from `fixtures/phase16/zod/matrix.json`. Each one is extracted
by CommitRift, evaluated by the model, and compared with Node.js running
zod 4.6.5 (as `zod` and `zod/v3`) and zod 3.25.76. The comparison covers
the decision, the parsed value (missing key, `undefined` and `null` are
distinct) and whether `parse` threw.

| | count |
| --- | ---: |
| comparisons | 1 220 |
| agreements | 943 |
| refusals (the model says "not modeled": the prototype-key schema and Map/Set inputs under zod 4; with no determinable zod version, every input on which zod 3 and zod 4 differ (68); zod 3.25's early `zod/v4` beyond the Phase 14 subset (204)) | 277 |
| **mismatches** | **0** |
| synthesized inputs (`zodfn::conform`) accepted by the real runtimes | 95 of 95 |

A refusal makes the candidate UNSUPPORTED. It is never a guessed accept.

## 3. Compatibility of the shared IR and reports

* **Model reports:** all 56 historical model scenarios, Phase 15 binary vs
  the integrated Phase 16 release: **56 of 56 byte-identical** (JSON and
  text; `scripts/phase10-model-identity.py`).
* **Static outputs:** `check`/`trace` on the Phase 8 corpus in the
  `shared`, `compact` and `legacy` modes: **144 of 144 identical**
  (`scripts/phase8-identity.sh`).
* Every new IR field is serde-defaulted and skipped when empty:
  * `Op::Try.inspect`;
  * `ManyRows`;
  * `TableSchema.nulls_not_distinct`;
  * the generated-identity `Formula` and `TermOp` variants.
  
  A Phase 15 scenario therefore loads unchanged and serializes
  byte-identically. The Phase 15 binary refuses a Phase 16 contracts file
  that uses a new formula (`unknown variant forEachActor`) instead of
  misreading it.

Raw: `reports/phase16/compat/`.

## 4. Test counts

| run | passed | failed | ignored | targets |
| --- | ---: | ---: | ---: | ---: |
| Phase 15 baseline (1efa383) | 751 | 0 | 3 | 71 |
| Phase 16 final, macOS and Linux (`COMMITRIFT_REQUIRE_REPLAY=1 COMMITRIFT_REQUIRE_SCHEMA=1 COMMITRIFT_REQUIRE_ZOD=1`; [details](QUALITY-GATES.md#10-final-gate-results)) | 843 | 0 | 3 | 83 |

The required-runtime switches make a test fail, not skip, when
PostgreSQL, Node.js, the zod runtimes or the schema validator are missing.

## 5. Known model approximations

| approximation | where stated | effect |
| --- | --- | --- |
| multi-row lock order without `.orderBy(key)` follows key order | candidate assumptions; [MULTI-ROW-LOCKS.md](MULTI-ROW-LOCKS.md#7-limitations) | a NO_VIOLATION is conditional on key-order locking (24 of 51 measured schedules differ) |
| loops: unrolled up to the established bound (or 8); beyond it the candidate is TRUNCATED/UNSUPPORTED, never safe | `loop …` assumptions ([LOOP-SEMANTICS.md](LOOP-SEMANTICS.md)) | bounded |
| caught errors: an error object's properties are modeled only for origins whose shape was verified (drizzle-orm 0.45 `DrizzleQueryError`, a thrown `Error`) | `caught-errors:` assumption | other `catch` inspections are UNSUPPORTED |
| zod: the documented subset; refinements, transforms, preprocess, coerce and unions are refused with the construct named | candidate first blocker | UNSUPPORTED |
| read replica: one asynchronous replica, unbounded lag | replica notes | counterexamples are model-only |
| generated identity on original code | `not-evaluated` | no Phase 16 property is reported held or violated on original code |

## 6. Unsupported constructs (by design, always reported)

The complete list with examples is in
[supported-patterns.md](../reference/supported-patterns.md#phase-16-additions-model-check-discover)
and [limitations.md](../reference/limitations.md#phase-16-changes). The
most frequent ones in measured code:

* JavaScript standard-library calls (`Set.prototype.has`,
  `Number.isSafeInteger`, `Math.min`);
* optional chains on call results;
* `Promise.all` with a used result;
* zod refinement callbacks;
* `for…in` and loops without an established bound;
* `instanceof` an application error class in a `catch`;
* recursive calls.

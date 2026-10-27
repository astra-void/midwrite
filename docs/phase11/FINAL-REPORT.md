# Phase 11 final report: automatic scenario synthesis

Phase 11 connects the Phase 6–10 verification machinery to unfamiliar
TypeScript: `commitrift discover` indexes a project, finds pairs of effects
that may interact under concurrency or failures, attaches properties whose
origin is always stated, synthesizes small valid database states with the
model's own engine, writes ordinary scenarios, model-checks them within
budgets, and explains the result. Original code runs only through the
established, separately authorized `model-check --verify-source
--allow-execution`. Every number below comes from a file under
`reports/phase11/`.

**Snapshot.** Base commit `accbcf5` (Phase 10, committed; clean tree, no
uncommitted Phase 10 work). Phase 11 changes are uncommitted. Two files
outside this work (`LICENSE-APACHE`, `LICENSE-MIT`, a wording change of the
copyright line) were modified in the working tree by someone else during
the phase; they were left as found. macOS:
Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11.
Nothing was committed, pushed or published.

## 1. Architecture (`ARCHITECTURE.md`)

* **Source indexing**: `commitrift-analyzer::entries` lists entry candidates
  (Server Actions, route handlers, exported functions, wrapped callbacks,
  class methods, object properties) as source selectors, with imports and
  transaction helpers. Table bindings come from a probe scenario loaded by
  the model checker's own loader; each entry is extracted with exactly the
  request a generated scenario's loader will make (`EFFECT-INDEX.md`).
* **Resource relationships**: effect summaries trace every value to a
  parameter path, literal, read column or computation; the
  `(table, column)` resource index drives pairing (`CANDIDATE-DISCOVERY.md`).
* **Property provenance**: `SCHEMA_ENFORCED`, `USER_DECLARED`,
  `FRAMEWORK_CONTRACT`, `HEURISTIC_HYPOTHESIS` (and `SOURCE_ASSERTED`,
  defined but not produced) with strengths; never upgraded
  (`INVARIANT-SYNTHESIS.md`).
* **Scenario generation**: arguments from parameter paths; rows found by
  running each actor alone in the engine and satisfying the reads that came
  back empty; foreign keys, joins, defaults, keys; serial outcomes computed
  by the engine; variants (actual, distinct-key control,
  distinct-request-key, hypothetical) (`STATE-SYNTHESIS.md`).
* **Model checking**: `report::check`, sleep sets by default, source-DPOR
  opt-in, budgets, counterexample validation (replay, violation, operations
  exercised) (`SOURCE-MODEL-FIDELITY.md`).
* **Runtime verification**: emitted scenarios carry `sourceVerification`
  from a declared harness mapping; running them needs `--allow-execution`.
* **Contracts**: `commitrift-contracts.json` for what source cannot say
  (`CONTRACTS.md`).

## 2. Correctness

### Quality gates (final tree)

Source digest (`scripts/phase8-digest.py`, 640 files):
`ad9499493ca541585dda5de9afff3b750943490fb8b628a2077f42a690cb91a4`;
release binary sha256 `0643249f1b9274ca…` (`reports/phase11/final/`).

| gate | macOS arm64 |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast` | exit 0: **583 passed, 0 failed, 3 ignored** (25.6 min, `reports/phase11/final/test.txt`) |
| `cargo build --release` | exit 0 |

583 = the 555 Phase 10 tests + 28 new. Ignored: the two Phase 10 tools
(`regenerate_fixtures`, `dump_seed`) and `dump_ir` (prints a project's
extracted IR). After the full run, one reporting change was made — the
category of an inconclusive candidate whose scenario does not load (it was
counted as "exploration budget exceeded") — and the affected suites were
rerun: `commitrift-discover` 19 passed, CLI `phase11` 6 passed; fmt and
clippy clean. The evidence (`reports/phase11/evidence/`) was produced by
the binary before that change (sha256 `6141d12d…`); the change affects only
`summary.unsupportedBreakdown` of saas-starter.

**Linux**: not run for Phase 11 (the Phase 10 container procedure exists,
`scripts/phase10-linux-docker.sh`; it was not executed). Cross-platform
identity is therefore not established for Phase 11.

### Compatibility

| | result |
| --- | --- |
| static analysis | **144 / 144** corpus outputs byte-identical to the Phase 10 binary (4 projects × depths 3, 6, 12 × 3 path modes × json, sarif, text, trace) — `reports/phase11/final/corpus-identity.txt` |
| model checking | **56 / 56** Phase 6–9 scenarios identical to the Phase 10 binary, JSON and text — `reports/phase11/final/model-identity.txt` |
| report schemas | model-check schemas 1–5 unchanged; discovery report is a new kind (`commitrift.discovery`, schema 1) |
| runtime verification | outcome terms are now evaluated after a run without crashes or retries; no Phase 7–10 source-verified scenario uses them, so no earlier result changes |
| analyzer | one fix in the Phase 7 extractor (no panic on a multi-byte character in a mislocated excerpt); the mislocation itself is kept for report identity |

### Generated-scenario validity

On the commerce application: 22 candidates, 31 scenarios built, 31
model-checked, 781 states in total; every counterexample replays strictly
and violates its invariant; every emitted scenario gives
`commitrift model-check` the same verdict and state count; every
synthesized state passes the loader's constraint checks and the source
foreign keys. Two pairing candidates are `UNSUPPORTED` (a helper returns a
mutation's rows).

### Counterexample replay on the original code

**10 of 10** generated counterexample scenarios with a harness mapping
reproduced on the unchanged original TypeScript and a disposable
PostgreSQL in **3 of 3** runs each (9 `original-code-reproduced`, 1
`original-code-crash-reproduced`): check-then-insert (`inviteMember`), lost
update (`reserveItem`, alone and against `reserveItemLocked`), double
claim (`claimTask`), double payout (`processRefund`, concurrent and
redelivered), double charge (`checkout`, concurrent; and crash + retry),
and the `checkoutIdempotent` false positive (its `charges` rows, and two
charges with different request keys). The **vertical slice** — a wrapped
Server Action, automatically discovered check-then-act, heuristic
property, synthesized member/organization state, model counterexample,
original-code reproduction — is the first row
(`automatically_discovered_counterexamples_reproduce_on_the_original_code`).

### Failed and inconclusive cases

* saas-starter `signUp`: `INCONCLUSIVE`, the scenario does not load (a local
  assigned only by an unsupported operation).
* commerce pairs with `reserveItemAtomic`: `UNSUPPORTED`.
* Baseline: 4 static tests depend on pre-built fixture `dist/` directories
  (pass once built), and one Node runtime test failed once under load
  (`BASELINE.md`).
* The first evidence run's script exit status was 1 because the script
  wrongly required `original-code-reproduced` for a crash counterexample;
  the script was corrected, the runs themselves all reproduced.

## 3. Real-world evaluation (`REAL-WORLD-EVALUATION.md`)

| project | entry candidates | extracted | effectful | candidates | counterexamples |
| --- | ---: | ---: | ---: | ---: | ---: |
| saas-starter | 44 | 37 | 8 | 2 | 1 (heuristic, model only) |
| midday | 2 864 | 1 168 | 0 | 0 | 0 |
| school-ledger | 880 | 546 | 0 | 0 | 0 |
| unnamed-ai-project | 74 | 33 | 1 | 0 | 0 |

* saas-starter's `inviteTeamMember` race is discovered with no scenario
  and no contract.
* Three projects yield nothing because the loader refuses their tables
  (generated `uuid`/`$defaultFn` primary keys: 42, 21, 2 tables), which
  leaves their database operations unbound. Unknown operations are
  categorized (method calls and package calls dominate).
* No corpus project was executed. **No independent human review was
  performed**; no corpus finding is claimed as a defect.

## 4. Accuracy (`PRECISION-RECALL.md`)

| benchmark | labels | TP | FP | FN | TN | precision | recall |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| first-party (same author, written before the engine) | 8 pos / 10 neg / 3 unsupported | 8 | 1 | 0 | 9 | 0.889 | 1.000 |
| held-out Phase 7/8, **first scoring** | 8 pos / 4 neg | 4 | 2 | 4 | 2 | 0.667 | 0.500 |
| held-out, after fixes prompted by it (not held out) | 8 pos / 4 neg | 7 | 0 | 1 | 4 | 1.000 | 0.875 |

Model prediction (validated counterexample of an entry's own candidate).
Static hazard prediction on the first-party set: precision 0.800, recall
1.000. Unsupported entries: 3 of 3 reported, 0 claimed safe. Class
agreement 7 / 9.

* **False positives**: a heuristic property broader than the labeled one
  (`checkoutIdempotent`: two `charges` rows for one deduplicated charge —
  a real inconsistency the label did not consider); before the fixes,
  serial outcomes that ignored legitimate 40001 failures.
* **False negatives**: requirements discovery does not synthesize (a
  committed state change whose event can be lost); before the fixes,
  extraction idioms and joined tables.
* **Unsupported patterns**: generated primary keys, unresolved method and
  package calls, unsupported expressions and binding patterns, barrel
  re-exports, transaction shapes, loops over unknown collections.

## 5. Performance (`PERFORMANCE.md`)

* `commitrift check`: unchanged (0.01 s commerce, 0.22 s vs 0.21 s midday).
* Discovery on commerce: 0.06 s; with model checking of 31 scenarios
  0.08 s; ≈ 10 MiB.
* midday (2 864 entry candidates): 30 s, 73 MiB, ≈ 98 % extraction; warm
  extraction cache 7.7 s, identical report.
* Runtime verification: 1.4–6 s per run, never part of `discover`.

## 6. Tests added

* analyzer: 2 (`entries.rs`), 1 regression (`concurrency.rs`);
* discover: 5 unit (contracts, effects, synthesis), 14 integration
  (labels, safe counterparts, provenance, validation, state validity,
  determinism, contracts off, metamorphic transformations, tenants,
  exclusive branches, read-only, atomic update, unknown SQL and calls,
  partial unique index, contradictions, cache invalidation);
* CLI `phase11.rs`: 6 (static exit codes, explanations, emitted-scenario
  equivalence, HTML escaping, authorization, original-code reproduction).

## 7. Limitations (`LIMITATIONS.md`)

Properties are hypotheses unless declared or schema-enforced; the
first-party benchmark is not independent and the held-out set was used
after its first scoring; no independent review; no external application
produced a verified finding; generated primary keys block most real
schemas; Linux was not run for Phase 11.

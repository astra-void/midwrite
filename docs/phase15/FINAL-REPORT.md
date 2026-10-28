# Phase 15 final report: counterexample-guided state synthesis, verification coverage and developer alpha stabilization

Every number comes from a file under `reports/phase15/`. Nothing was
committed, pushed or published.

**Snapshot.** Branch `phase-8`, base commit `4c338ad` (the Phase 14 final
tree, digest `e486816b…`, verified identical at the start:
`BASELINE.md`); Phase 15 changes uncommitted. macOS: Darwin 27.0.0 arm64,
rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11. Final source digest
(`scripts/phase8-digest.py`, 927 files):
`ced40150be07522b7b79319e9c92aff4f63c157d4e5b1bdbaa17370a101a2264`.

Documents (several of the brief's documents are consolidated):

| brief | document |
| --- | --- |
| BASELINE | `BASELINE.md` |
| ARCHITECTURE | `ARCHITECTURE.md` |
| INPUT-DOMAINS | `INPUT-DOMAINS.md` |
| CONSTRAINT-SYNTHESIS | `CONSTRAINT-SYNTHESIS.md` |
| STATE-SPACE-SAMPLING, FEEDBACK-GUIDED-SEARCH | `FEEDBACK-GUIDED-SEARCH.md` |
| VERDICT-SEMANTICS, INPUT-COVERAGE | `VERDICT-SEMANTICS.md` |
| SOURCE-COVERAGE | `SOURCE-COVERAGE.md` |
| MIDDAY-EVALUATION, SCHOOL-LEDGER-EVALUATION | `REAL-WORLD-EVALUATION.md` |
| POSTGRESQL-DIFFERENTIAL | `POSTGRESQL-DIFFERENTIAL.md` |
| PRECISION-RECALL | `PRECISION-RECALL.md` |
| PERFORMANCE | `PERFORMANCE.md` |
| ALPHA-READINESS | `ALPHA-READINESS.md` |
| SECURITY | `SECURITY.md` |
| LIMITATIONS | `LIMITATIONS.md` |

## 1. Implementation

| area | what was built | code |
| --- | --- | --- |
| input domains (A) | `PathDomain` per parameter path: kind (16 kinds), `null`/`undefined` permission from the annotation and optionality only (else UNKNOWN, never generated), provenance, value classes (null, undefined, branch operands, integer boundaries, negation); string-literal unions resolved across modules | `domains.rs`; `TsActorProvider::param_signature` |
| constraint-aware synthesis (B) | `build_world`: argument overrides before rows are synthesized, cell overrides after the actors' first solo runs, every actor must still reach the candidate alone; guards over read rows and over arguments satisfied with stated preconditions; typed distinct values (enum, varchar length, uuid, time); where an actor stopped is reported | `synth.rs`, `worlds.rs` |
| feedback-guided search (C) | per actual variant: domain, actor, relationship (stored = / ≠ written) and stored-boundary worlds; priority order; branch outcomes of solo runs (from the engine's `CHECK` events, including the local prefix via `initial_state_recorded`) move worlds over one-way branches first; stop at the first validated counterexample | `pipeline.rs`, `worlds.rs`, `engine.rs` |
| deduplication | fingerprint of the whole scenario document minus name/description | `worlds::fingerprint` |
| coverage metadata (D) | per candidate: source semantics, initial states, actor inputs, schedules, branch outcomes, every world's outcome, `domainExhaustive` (always false), `classesExhaustive`, unexplored list, conclusion; finding texts state the bounds | `pipeline.rs` `coverage_of`, `report.rs` |
| source support (E) | literal-union aliases; optional members; declared-only `guarded-update` family; declared `atMostOne` keys from the INSERT's written value | `js_values.rs`, `detect.rs` |
| CLI (G) | `--max-worlds N` (default 8; 1 = off), `--all-worlds`, `--explain-coverage`; coverage in text, JSON and HTML; `*.counterexample.json` reproduction artifacts with `--emit-scenarios` | `commitrift-cli/src/discover.rs` |
| first-party application | `fixtures/phase15/worlds-app`: NULLS DISTINCT key reached only with `parentId = null`, quota guard reached only at stored `remaining = 1`, each with a safe counterpart, an authorized harness | fixture |
| fresh evaluation set | `fixtures/phase15/eval-app` (30 entries), frozen by an independent AI author; scorer `scripts/phase15-eval.py` | fixture, script |
| scripts | `phase15-evidence.py`, `phase15-perf.py`, `phase15-corpus.py`, `phase15-linux-docker.sh` | `scripts/` |

## 2. Correctness

* **E21 (Phase 14 set)**: found through the generic value-distinction
  relationship: base world `NO_COUNTEREXAMPLE`, world
  `stored-distinct:accounts.displayName` a validated counterexample to the
  declared `display-name-read-your-writes` (USER_DECLARED). Model only (a
  replica model). Permanent test
  `e21_a_stored_value_distinct_from_the_written_one_exposes_the_stale_read`.
* **E26 (Phase 13 set)**: found through the nullable domain of
  `parentId: number | null`: world `null:parentId` for both actors, the
  `parentId === null` branch flipped (branch outcomes 3 → 5 of 8),
  counterexample validated. Test
  `e26_a_null_argument_permitted_by_the_signature_reaches_the_duplicate`.
  The same mechanism, on the first-party worlds application, **reproduces
  on the original code 3/3**.
* **PostgreSQL differential**: 28 of 31 counterexample worlds confirmed in
  lockstep, 166 outcomes, 0 disagreements (3 replica models not
  replayable). Original code: 2 input-dependent defects 3/3 each; the
  safe NULLS NOT DISTINCT counterpart not reproduced 0/3
  (`POSTGRESQL-DIFFERENTIAL.md`).
* **POR agreement**: 98 of 98 generated worlds agree on verdict and
  completeness under the unreduced search, sleep sets, classic DPOR and
  source-set DPOR.
* **False positives found and removed during the work** (never in a
  scored Phase 15 result):
  1. actor worlds gave two redeliveries of one job different job ids
     (Phase 13 set E25 became a FP); now only independent-request classes
     without a retry contract get actor worlds;
  2. a declared key instantiated from the INSERT made a NULLS NOT DISTINCT
     table a model counterexample the model cannot rule out; such
     counterexamples are now INCONCLUSIVE (PostgreSQL confirmed the
     original code holds);
  3. synthesized arguments outside a string-literal union (`"kind-1"` for
     `"page" | "redirect"`) are no longer generated.
* **Changes to earlier outputs** (intentional): base-world arguments
  typed by a string-literal union now take a member; guards that
  synthesized inputs failed are now satisfied (school-ledger: 7
  INCONCLUSIVE → UNSUPPORTED with a later blocker); candidates gain the
  coverage section and finding sentences; discovery reports gain additive
  fields. No Phase 11–14 test expectation had to change; the only edit to
  an existing test file excludes `fixtures/phase15` from the analyzer's
  fixture-annotation scan (as every earlier phase directory is).
* **Unsupported assumptions / known approximations**: worlds vary one
  relationship at a time; branch coverage is from solo runs; nullability
  needs an annotation; synthesized authorization preconditions choose
  callers that pass the guards (stated, not assumed of all callers); no
  unsound approximation was introduced — every world is checked by the
  unchanged model, and every new reason for a result is stated in it.

## 3. Real-world coverage (Phase 14 → Phase 15)

| project | metric | Phase 14 | Phase 15 |
| --- | --- | ---: | ---: |
| saas-starter | effectful / candidates / model-checkable / counterexamples | 9 / 2 / 1 / 1 | 9 / 2 / 1 / 1 |
| midday (declared factory) | effectful / resources / candidates | 81 / 123 / 4 | 81 / 123 / 4 |
| midday | model-checkable / statuses | 0 / 1 INCONCLUSIVE, 3 BLOCKED | 0 / unchanged (blockers: properties over generated ids; undeclared multi-row invariants) |
| school-ledger | effectful / candidates | 70 / 9 | 70 / 9 |
| school-ledger | model-checkable / statuses | 0 / 7 INCONCLUSIVE, 2 UNSUPPORTED | 0 / **9 UNSUPPORTED** (7 now pass their authorization guards with stated preconditions and stop at zod `strictObject`) |
| corpus wall / RSS | midday declared | 1.36 s / 144 MiB | 1.32 s / 148 MiB |

No external project was executed; no external defect is claimed
(`REAL-WORLD-EVALUATION.md`).

## 4. Evaluation

| set | scoring | TP | FP | FN | TN | precision | recall |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| **Phase 15 set** (fresh, frozen) | Phase 14 binary | 2 | 0 | 11 | 14 | 1.000 | 0.154 |
| | **first scoring (held out)** | **4** | **0** | **9** | **14** | **1.000** | **0.308** |
| | post-feedback final (not held out) | 8 | 0 | 5 | 14 | 1.000 | 0.615 |
| Phase 14 set (regression) | Phase 14 → 15 | 4 → **5** | 0 → 0 | 14 → 13 | 18 → 18 | 1.000 | 0.222 → 0.278 |
| Phase 13 frozen set (regression) | Phase 14 → 15 | 7 → **8** | 0 → 0 | 8 → 7 | 11 → 11 | 1.000 | 0.467 → 0.533 |
| Phase 12 set, Phase 11 sets | Phase 14 → 15 | unchanged | unchanged | unchanged | unchanged | | |

Coverage failure categories of the remaining Phase 15 FNs: missing
candidate 1 (E03), unsupported source behavior 3 (E06, E10, E24), missing
invariant 1 (E20); none from missing input values, invalid rows, unreached
branches or budgets. Independent review: **none**; the set's author was an
AI agent without access to the implementation (`PRECISION-RECALL.md`).

## 5. Performance

World search costs +0–30 % wall time on the evaluation applications (e.g.
Phase 15 set 0.059 → 0.075 s), no measurable memory; every non-base
counterexample came from the first or second derived world; worlds are
5–40 states each; corpus timings unchanged within noise
(`PERFORMANCE.md`).

## 6. Quality gates and compatibility

Run on a frozen `rsync` copy of the final tree (digest `ced40150…`) with
its own `CARGO_TARGET_DIR`. Logs: `reports/phase15/final/`.

| gate | result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast` | **751 passed, 0 failed, 3 ignored** (71 test targets; baseline 734 / 0 / 3 + 17 new) |
| `cargo build --release` | exit 0, sha256 `903356e5…` |
| historical model reports (`scripts/phase10-model-identity.py`, Phase 14 binary vs final) | **56 of 56 identical** |
| static `check`/`trace` outputs (`scripts/phase8-identity.sh`, depths 3/6/12, modes shared/compact/legacy, 4 corpus projects) | **144 of 144 identical** |
| POR regressions, adversarial POR, state storage, FK locks, replicas, response loss, generated-value identity | in the test suite above |
| Phase 15 evidence (`scripts/phase15-evidence.py --repeat 3`) | exit 0: 98/98 POR agreement, 28/31 lockstep confirmed (0 disagreements), 2 original-code reproductions 3/3 |
| documentation links (`scripts/check-doc-links.py`) | 234 files, **0 broken** |
| determinism | discovery JSON byte-identical across runs (test) |

New tests: 9 in `crates/commitrift-discover/tests/phase15.rs`, 3 in
`crates/commitrift-cli/tests/phase15.rs` (one needs PostgreSQL and Node),
5 unit tests in `domains.rs`/`worlds.rs`.

## 7. Developer alpha

`ALPHA-READINESS.md`. Installation: build from source. Quickstart
(`docs/getting-started/quickstart.md`, "Explore input worlds") validated
with the final binary end to end, including original-code reproduction.
Supported patterns and limitations: `docs/reference/supported-patterns.md`,
`docs/reference/limitations.md` (Phase 15 section). Release blockers:
zero model-checkable candidates on the larger real projects, moderate
recall on fresh cases, no independent review, replica counterexamples
model-only, Windows untested. Recommendation: closed developer alpha on
first-party code in the supported subset; not an open release.

## 8. Platforms

**Linux was run** (first time since Phase 12): Docker Desktop's daemon
was stopped at the start of the cycle and came up when started once;
`scripts/phase15-linux-docker.sh` ran the gates and every evidence script
in a Debian 12 container (aarch64, rustc 1.99.1, Node 22.23.4, PostgreSQL
15.19, bubblewrap 0.8.0) on a copy whose digest is **`ced40150…`, the same
as the macOS run**. Logs: `reports/phase15/linux/`.

| gate (Linux) | result |
| --- | --- |
| `cargo fmt --check` | exit 0 |
| `cargo test --workspace` with required replay | **751 passed, 0 failed, 3 ignored** (71 targets) |
| release build | exit 0 |
| Phase 12, 13, 14 evidence scripts (`--repeat 3`) | exit 0 each |
| Phase 13 lock matrix | exit 0 |
| **Phase 15 evidence** | exit 0: 98/98 POR agreement, 28/31 lockstep confirmed (0 disagreements), `createFolder` and `startExport` reproduced on the original code 3/3, `createLabel` not reproduced 0/3 — identical to macOS (PostgreSQL 15.19 vs 17.11) |
| `cargo clippy -D warnings` | **exit 101** in this run: one `needless_borrows_for_generic_args` lint in `crates/commitrift-model/src/engine.rs:1181` (`map(&close)`), code unchanged since before Phase 15; the container installs the latest stable toolchain (rustc 1.99.1), whose clippy has this lint, while the project's macOS toolchain (1.98.1) passes. **Fixed after the run** (§8a) |
| Phase 13 evidence schema check | exit 1: `reports/phase13/evidence/workspace-app-discovery.json` has no published schema (`commitrift.discovery`); the same check fails identically on macOS (pre-existing glob over a committed file, not a regression) |

### 8a. Post-freeze fix

One line, `map(&close)` → `map(close)` in `engine.rs:1181` (the closure
captures only a reference and is `Copy`; no behavior change). Rechecked on
the fixed tree (digest
`98567599d96cefdc985b3f0b380f2a14276dd46a838cd08d8cf9c2526f9be045`, 927
files): macOS `cargo fmt --check` and `cargo clippy --workspace
--all-targets -D warnings` exit 0; `COMMITRIFT_REQUIRE_REPLAY=1 cargo test
-p commitrift-model` 196 passed, 0 failed, 2 ignored (23 targets;
`reports/phase15/final/model-tests-after-fix.txt`); in the Linux container
(rustc and clippy 1.99.1) `cargo fmt --check` and `cargo clippy -D
warnings` exit 0 (`reports/phase15/linux/clippy-after-fix.txt`). The full
workspace suites of §6 and §8 ran on the tree before this line changed
(`ced40150…`); they were not repeated.

Windows was not run.

## 9. Definition of done

| # | item | state |
| --- | --- | --- |
| 1 | Phase 1–14 tests remain valid | **751 passed, 0 failed, 3 ignored** (71 test targets; baseline 734 / 0 / 3 + 17 new)_SHORT |
| 2 | typed input domains | done (`domains.rs`) |
| 3 | equal/distinct value relationships | done (`stored-distinct`/`stored-equal`, actor worlds, key variants) |
| 4 | null arguments where valid | done (annotation-gated) |
| 5 | states satisfy modeled constraints | done (loader-checked; unmodeled ones reported) |
| 6 | multiple deterministic worlds | done |
| 7 | feedback-guided exploration | done (branch feedback, stop on validated counterexample) |
| 8 | E21 via the generic mechanism | done |
| 9 | E26 via the nullable domain | done |
| 10 | input and schedule coverage separate | done |
| 11 | incomplete input exploration never presented as safety | done (finding text, `domainExhaustive`) |
| 12 | reproducible counterexample artifacts | done (`*.counterexample.json`) |
| 13 | school-ledger's nine re-evaluated | done (0 model-checkable; precise new blocker) |
| 14 | midday's four re-evaluated | done (0 model-checkable; blockers restated) |
| 15 | a new original-code PostgreSQL verification | done (2 slices, 3/3 each) |
| 16 | fresh frozen evaluation, first score preserved | done |
| 17 | historical benchmarks traceable | done |
| 18 | POR regressions pass | done (suite + 98/98) |
| 19 | cache invalidation covers input domains | done (verdicts never cached; test) |
| 20 | reports explain the bounds | done |
| 21 | installation and quickstart validated | done |
| 22 | macOS full regression | **751 passed, 0 failed, 3 ignored** (71 test targets; baseline 734 / 0 / 3 + 17 new)_SHORT |
| 23 | Linux full regression or honest blocker | done: tests 751/0/3 and all evidence on Debian 12 (same digest); the one clippy lint of the container's newer toolchain fixed and rechecked on both platforms (§8a) |
| 24 | reproducible evidence | done (`reports/phase15/`) |
| 25 | no unsupported behavior silently converted into safety | done |

# Phase 14 final report: real-world source coverage and developer alpha readiness

This cycle had three parts: close Phase 13, consolidate the documentation,
and implement Phase 14 (source coverage, verification precision, developer
usability). Every number comes from a file under `reports/phase13/` or
`reports/phase14/`. Nothing was committed, pushed or published.

**Snapshot.** Branch `phase-8`, base commit `32efbca`; Phase 13 and Phase 14
changes uncommitted. macOS: Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2,
PostgreSQL 17.11 (Homebrew). Final source digest (`scripts/phase8-digest.py`,
879 files): `e486816bed5cb6a1cc11aa1e1d710239b8a6cf222e97a2510f2544dc8e1b44cb`;
release binary sha256 `1f035c611112ea52faf9b7fca91eac9b6309b6f3df9339c21c0e2b23127a65dd`.

Documents: `BASELINE.md`, `ARCHITECTURE.md`, `VALUE-SEMANTICS.md`,
`REPLICA-INTEGRATION.md`, `SOURCE-COVERAGE.md`, `DRIZZLE-DIFFERENTIAL.md`,
`EVALUATION.md`, `REAL-WORLD-RESULTS.md`, `PERFORMANCE.md`, `SECURITY.md`,
`LIMITATIONS.md`, this report.

## 1. Documentation

* **New structure** (`docs/README.md` is the index): `getting-started/`,
  `user-guide/`, `concepts/`, `architecture/`, `reference/`, `development/`,
  `research/` — 35 current pages describing the system as it is, linking to
  the phase records for detail. `docs/reference/limitations.md` is the single
  current limitations list (supported+verified, model-only, approximate,
  unsupported source, unsupported PostgreSQL, drivers, missing independent
  verification; a "lifted by later phases" table; the Phase 14 changes).
  `docs/research/evidence-index.md` lists every major benchmark with its
  phase, environment, revision, baseline, result, scope and raw path.
* **Historical files preserved**: no `docs/phaseN/` file was deleted or
  moved and no historical number was changed; Phase 13 documents were
  completed and corrected at its closeout, with the corrections marked.
* **Link validation**: `scripts/check-doc-links.py` (relative links and
  heading anchors; README, CONTRIBUTING, `docs/**`): at the end of Phase 14, 219 files checked, **0 broken links** (README/CONTRIBUTING, 38 current pages, 179 phase documents including Phase 14).
* **Migration record**: `docs/MIGRATION.md`.
* **CLI examples** were taken from real runs; the quickstart now walks
  through `discover` with Phase 14 output (`getting-started/quickstart.md`).

## 2. Phase 13 closeout

`docs/phase13/FINAL-REPORT.md` is complete: source digest `dc3161e1…`
(780 files), release binary `97e96ac0…`; fmt and clippy clean; full suite
with required replay **652 passed / 1 failed / 3 ignored** (run 1) and
**653 / 0 / 3** (run 2, same tree); release build OK. The one failure was a
load-dependent race in the original-code coordinator (a waiting statement
answered while the coordinator waited for the lock holder's COMMIT, reported
as a divergence although every result agreed); its cause was found and
fixed during Phase 14 (§5). The Phase 13 evidence script had never completed
(a `/` in a selector used as a file name); fixed and run: every slice
reproduced 3/3 except the Server-Action foreign-key pair (2/3, the same race).
Compatibility with the final Phase 13 binary: 144/144 static outputs and all
Phase 6–9 model reports (except a memory estimate) identical. Corrected
stale statements: replica plumbing (it was merged; midday's real blocker was
`nanoid`), DRAFT labels, section references, the observable-positive count.
Linux was not run (Docker daemon unavailable).

## 3. Phase 14 implementation

| area | what is now modeled | document |
| --- | --- | --- |
| value operations | bounded arrays, template literals and string methods, `undefined` ≠ `null`, `Map`/`Set`; synthesis types array/string arguments or refuses | `VALUE-SEMANTICS.md` |
| higher-order calls | closures passed to project functions, inlined where called with their captured scopes; ambiguous commit of a transaction retried by a `catch` | `SOURCE-COVERAGE.md` |
| validation | zod `parse`/`safeParse` for a documented subset, in zod 3's check order | `SOURCE-COVERAGE.md` |
| Drizzle / PostgreSQL | multi-row `INSERT`, `inArray`/`notInArray`, conditional predicates, timestamp and JSON subsets, `nanoid()`/`randomUUID()` | `DRIZZLE-DIFFERENTIAL.md` |
| replica round trip | discovery → scenario → loader effect traces compared before model checking | `REPLICA-INTEGRATION.md` |
| candidate families | `parent-child`; `return-value` (declared only; templates `atMostOneSuccess`, `exactlyOneSuccess`, `everySuccessReturns`); redelivered inserts; partial unique index → INCONCLUSIVE | `SOURCE-COVERAGE.md` |
| CLI | a next step on every candidate (text, JSON, HTML); `explain` lists CP7103; deterministic output re-checked | §7 |

Defects fixed along the way: generated keys at connection-loss and
server-completion checkpoints (any table with a generated key "differed");
an undetermined local evaluation surfacing as a catchable exception; serial
outcomes that are booleans or generated values turned into `null` (false
counterexamples); time/JSON columns filled with text in synthesized rows.

## 4. Real-world evaluation

| metric | Phase 13 | Phase 14 |
| --- | ---: | ---: |
| Midday effectful entries (declared factory) | 47 | **81** |
| Midday model-checkable candidates | 0 | **0** (the replica candidate now loads and runs; its heuristic property is not statable over generated ids: INCONCLUSIVE) |
| School-ledger candidates | 9 | 9 |
| School-ledger effectful entries | 21 | **70** |
| School-ledger model-checkable candidates | 0 | **0** (7 INCONCLUSIVE, 2 UNSUPPORTED; blockers in `REAL-WORLD-RESULTS.md`) |
| Phase 13 frozen set TP | 4 (post-feedback) | **7** (post-feedback) |
| Phase 13 frozen set FN | 11 (post-feedback) | **8** |
| Phase 13 frozen set FP | 0 | 0 |
| Phase 14 set, first scoring | — | TP 0 FP 0 FN 18 TN 18 (recall 0.000) |
| Phase 14 set, post-feedback 2 (not held out) | baseline TP 2 FP 1 FN 16 TN 17 | TP 4 FP 0 FN 14 TN 18 (precision 1.000, recall 0.222) |
| Original-code reproductions (first-party) | request-app slices 2, 3, E and the FK pair | + **2 new**: multi-row INSERT after a lost response (3/3); COMMIT lost inside a retry wrapper (3/3, with a stated client-error handler; diverged without it) |
| Discovery time, midday declared | 1.12 s / 121 MiB | 1.24 s / 148 MiB |

Details and causes of every remaining false negative: `EVALUATION.md`.
Earlier benchmarks (Phase 11 first-party and held-out, Phase 12 set) keep
their model scores.

## 5. Correctness

* **Differential tests** (PostgreSQL 17.11, every model schedule in
  lockstep): multi-row inserts 420 schedules, predicates 159, timestamps and
  JSON 90 — **0 disagreements** besides the known one-way SSI granularity
  case; Drizzle's own SQL (probe, `.toSQL()`) and the oracle's rendering
  select identical rows for 11 predicate forms.
* **Model/runtime agreement**: the two new original-code slices reproduce;
  with node-postgres's default behavior (no client error handler) the
  retry-wrapper slice **diverges** (the worker dies), which shows the
  model's "connection lost, process alive" assumption is environment
  dependent — stated in the harness and the limitations.
* **POR agreement**: sleep sets, classic DPOR and Source-DPOR agree with the
  unreduced search on 86 Drizzle models (344 runs) and 9 value-domain models
  × 3 levels; the independence audit is clean.
* **Compatibility**: Phase 6–9 model reports byte-identical to the Phase 13
  binary (57/57) on the integrated build; static `check`/`trace` 144/144
  identical; with the new features off, the reachable-state digests of 83
  earlier fixture scenarios were identical (measured in the value-domain
  work before integration) and 17 kept byte-identical reports and state
  bytes (Drizzle work, before integration); the full suite passes on the
  integrated tree.
* **Unsupported semantics** are refused by name (`LIMITATIONS.md`); an
  undetermined evaluation ends a path as unsupported.
* **False positives**: none on the Phase 14 set after post-feedback 2 (the
  one in post-feedback 1 came from an unmodeled partial unique index, now
  INCONCLUSIVE); none on the Phase 13 set. **False negatives**: listed by
  cause in `EVALUATION.md`, including a synthesis limitation that produced
  a NO_VIOLATION verdict for a true positive (E21).
* **Runtime harness**: the Phase 13 coordination race is fixed (the
  foreign-key slice passed 24/24 under parallel load after the fix, 1 of 6
  failed before it).

## 5a. Changes to earlier results (documented corrections)

* **Phase 11 test `partial_unique_indexes_do_not_count_as_protection`**
  expected MODEL_COUNTEREXAMPLE on a table whose partial unique index
  (`WHERE deleted IS NULL`) applies to the inserted rows, i.e. a duplicate
  PostgreSQL would reject. Phase 14 reports it INCONCLUSIVE (the index is
  still not a protection; the model does not enforce it). The test now
  expects that. It was the only failure of the first final gate run.
* **Phase 12 evaluation**: `importTasks` (labeled unsupported: "a loop over
  an input array of unknown length") is now modeled with a bounded
  synthesized array, so its scorer counts it "missed"; the frozen label was
  not changed.
* **Discovery output**: synthesized scenarios carry the `js-values`,
  `timestamps` and `json` features when they need them (the Phase 12
  workspace-app foreign-key scenario now lists `["foreign-keys",
  "js-values"]`; its original-code run still reproduces); the discovery
  cache version is `commitrift-discover-cache-4`; time and JSON columns are
  modeled in discovery (effectful-entry and resource counts rose on the
  corpus); every candidate has `nextStep`.
* **Engine**: an undetermined evaluation in a local step (including the
  Phase 12 refusal to order generated values) now ends the path as
  unsupported instead of failing the actor; no Phase 6–13 fixture scenario
  changed (§5).
* **Phase 13 statements** corrected at its closeout are listed in its final
  report.

## 6. Quality gates (final)

Run on frozen copies of the tree in a private scratch directory with their
own `CARGO_TARGET_DIR` (logs: `reports/phase14/final/`, run 1 in
`reports/phase14/final/run1/`).

| gate | run 1, tree `88e074f4…` | run 2, tree `e486816b…` (run 1 + the corrected test expectation of §5a) |
| --- | --- | --- |
| `cargo fmt --all -- --check` | exit 0 | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast` | 733 passed, **1 failed** (§5a), 3 ignored (69 test targets) | **734 passed, 0 failed, 3 ignored** (69 test targets) |
| `cargo build --release` | exit 0, sha256 `1f035c61…` | exit 0, sha256 `1f035c61…` (same binary) |
| `scripts/phase14-evidence.py --repeat 3` | all 4 slices as expected | all 4 slices as expected: multi-row INSERT 3/3 `original-code-crash-reproduced`, retry wrapper 3/3 `original-code-connection-loss-reproduced`, both safe counterparts hold |
| evidence schema validation | 0 errors | 0 errors |

Also run on this tree's build: compatibility with the Phase 13 binary
(57/57 model reports, 144/144 static outputs identical,
`reports/phase14/compat/`), link check (0 broken), output determinism (JSON,
text, HTML and emitted scenarios byte-identical across two runs on two
fixtures). The evaluation and corpus measurements (§4) used a release build
of the tree before `cargo fmt` and the §5a test change, which do not change
behavior.

**Platforms.** macOS arm64 only. Linux was not run: the local Docker daemon
answered API requests with HTTP 500 and later hung during this cycle; `scripts/phase14-linux-docker.sh` is ready (it adds the Phase 14
fixture and evidence to the Phase 13 container run). Windows was not run.

## 7. Release readiness (developer alpha)

| aspect | state |
| --- | --- |
| installation | build from source (`docs/getting-started/installation.md`); no packages or releases were published |
| quickstart | validated with the Phase 14 binary: `check` on one file, `discover --model-check` on a first-party app, `model-check` of an emitted scenario (`getting-started/quickstart.md`) |
| command discoverability | `commitrift --help` lists 10 subcommands; `explain` lists every code |
| useful errors | unsupported operations name the construct and the source location; every candidate has a next step |
| output stability | JSON, text, HTML and emitted scenarios byte-identical across runs on two fixtures; HTML script-free |
| configuration effort | discovery works without configuration, but useful properties need `commitrift-contracts.json` declarations; a replicated database needs a declared factory |
| performance | about 1.2 s and 150 MiB for midday (2 022 modules) |
| unsupported explanations | per-candidate first blocker; per-project unknown-operation categories |

**Alpha blockers** (honest list):

1. Recall on realistic code is low (0.222 on the new set after feedback;
   0 model-checkable candidates on midday and school-ledger).
2. Synthesis can hide violations (coinciding initial values) and does not
   explore `null` arguments; a NO_VIOLATION verdict is weaker than it reads.
3. Linux was not validated in this cycle; Windows never.
4. No independent human review of any evaluation.
5. Framework surfaces (request objects, `for` loops, time arithmetic, JSON
   access) are unsupported and common in real applications.
6. Original-code verification covers first-party fixtures only, and its
   connection-loss mode depends on how the server handles driver errors.

## 8. Commit status

Nothing committed, pushed or published. All Phase 13 and Phase 14 work is
uncommitted in the working tree on branch `phase-8` (base `32efbca`).

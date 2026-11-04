# Phase 16 Closed Alpha readiness

(Track G's G6–G7, checkpoint 9 and the brief's §8.6.)

The invited-developer package is [docs/alpha/](../alpha/README.md):
* the guide;
* a [feedback template](../alpha/feedback-template.md);
* a [bug-report template](../alpha/bug-report-template.md).

It is prepared and **not distributed**. Nothing was published, pushed or
sent.

## Walkthrough validation

`scripts/phase16-alpha-check.sh reports/phase16/alpha` runs every command
of the guide from a clean output directory and checks what the guide says
each one shows. Result: **every documented step behaved as described**
(`reports/phase16/alpha/alpha-check.log`):

| step | check | result |
| --- | --- | --- |
| 1. build | `cargo build --release -p commitrift-cli`; `commitrift --version` | ok (`commitrift 0.1.0`) |
| 2. first run | discovery never executes code; `CR-SYNTH-65F47A [MODEL_COUNTEREXAMPLE]`; conflict at `src/services/orders.ts:16:13` | ok |
| 3. property origin | `USER_DECLARED` and `SCHEMA_ENFORCED` labeled | ok |
| 4. coverage | `Input domain exhaustive: no`; schedule exploration; `loop BOUNDED_UNROLLING` | ok (see below) |
| 5. reproduce in the model | scenario and `.counterexample.json` written; `model-check` shows the violated declared property | ok |
| 6. unsupported behavior | loop shapes outside the subset named | ok |
| 7. JSON report | validates against `docs/phase16/schemas/discovery-report.v1.schema.json` | ok (0 errors) |
| 8. original code (optional, authorized) | `--verify-source --allow-execution --source-repeat 2` | ok: reproduced |

**Found and fixed by the walkthrough.** Step 4 failed at first. The guide
promised that `--explain-coverage` states how each loop was bounded, but
the text output printed assumptions only with `-v`; the loop bound was in
the JSON only. A loop bound is coverage information, so
`--explain-coverage` now prints the `loop` assumptions too
(`crates/commitrift-discover/src/report.rs`), covered by the CLI test
`explain_coverage_states_how_each_loop_was_bounded`. The CLI reference row
was updated.

**Quickstart.** Every example of
[getting-started/quickstart.md](../getting-started/quickstart.md) was run
with the final release binary (`reports/phase16/alpha/quickstart.log`):
* `check`, `explain`, `trace` and the fixture totals (36 files, 37
  warnings, 15 info; safe: 0 warnings, 1 info);
* Phase 14 discovery, emit and model-check (the 9-transition schedule,
  `count(payments[orderId=1]) = 2`);
* Phase 15 world exploration (`null:parentId` counterexample, 3 worlds, 5
  of 8 branch outcomes), the artifact, the model replay and the authorized
  original-code run (reproduced, 3 of 3).

One statement was wrong: the Phase 14 section said discovery exits 0 after
reporting a counterexample. It exits 1, as the user guide says, and the
quickstart now says so.

## Developer-experience changes in Phase 16 (G6)

* **Unsupported explanations.** Repeated unsupported constructs are grouped
  once with all their locations. Each candidate names its first blocker
  with `file:line:col`.
* **Property declaration help.** BLOCKED and INCONCLUSIVE candidates print
  a concrete declaration skeleton. The INCONCLUSIVE next step names the
  real contract fields (`entries` → `values` / `arguments`), not a
  non-existent `initialState` (fixed at integration).
* **Reproduction commands.** A MODEL_COUNTEREXAMPLE prints the exact
  `model-check DIR/<file>` command for the `--emit-scenarios` directory
  given, and labels the `--verify-source --allow-execution` command as
  executing application code.
* **Coverage.** `--explain-coverage` lists every world, every gap and every
  loop bound.
* **Deterministic output.** Two runs print identical text (tested).
* **Report schema.** A published discovery-report JSON Schema exists, with
  positive and negative tests ([QUALITY-GATES.md](QUALITY-GATES.md)).
* **Execution controls are unchanged.** Nothing executes without
  `--allow-execution` and a user-written harness.

## §8.6 summary

| item | state |
| --- | --- |
| installation experience | one `cargo build --release -p commitrift-cli` (Rust 1.97+); no runtime needed for discovery or model checking; Node 22 and PostgreSQL server binaries only for the optional original-code step. No packaged binary: by design, nothing is published |
| quickstart validation | every example runs as documented (above) |
| supported real-world patterns | Next.js route handlers with `request.json()`, exported server functions, Drizzle 0.45 on PostgreSQL 15+, zod 3/4 object validation (documented subset), bounded retry loops, multi-row locks, NULLS NOT DISTINCT, generated identifiers, a declared read replica ([supported-patterns.md](../reference/supported-patterns.md#phase-16-additions-model-check-discover)) |
| accuracy limitations | Phase 16 frozen set: precision 1.000, recall 0.333 (6 of 18 positives found; 0 false positives among 19 negatives). On real projects most candidates still stop at an unsupported construct (school-ledger 0 of 9 model-checkable, midday 1 of 4). One historical false positive against a heuristic property (Phase 12 `transferCredits`) ([PRECISION-RECALL.md](PRECISION-RECALL.md)) |
| evidence interpretation | the guide's status table and [evidence levels](../concepts/evidence-levels.md); every report distinguishes declared, schema-enforced and hypothesized properties, and model-only from PostgreSQL-confirmed and original-code evidence |
| security considerations | [SECURITY.md](SECURITY.md); the guide's step 8 is opt-in and points to the security instructions first |
| external review readiness | ready for invited developers: reproducible commands, templates, review packet ([PRECISION-RECALL.md](PRECISION-RECALL.md#review-packet)). **No independent human review has been performed** |
| release blockers | (1) no independent human review of labels or results; (2) recall on realistic code is low: JavaScript standard-library calls and zod refinements are the most frequent blockers; (3) multi-row lock order without `ORDER BY` is approximate; (4) no packaged binaries or install channel (intended for a closed alpha); the macOS and Linux gates both pass on the same source ([QUALITY-GATES.md](QUALITY-GATES.md#linux)) |

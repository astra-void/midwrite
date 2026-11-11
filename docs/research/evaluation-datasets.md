# Evaluation datasets

The labeled first-party applications that score `midwrite discover
--model-check`, under `fixtures/eval/`. Each set was written to probe one
generation of the supported subset; each is kept, frozen, and re-scored as
a regression check. How sets are scored and what the scores may support:
[evaluation methodology](evaluation-methodology.md). Measured scores:
[evidence index](evidence-index.md). Static `check` precision and recall on
third-party projects is a separate harness:
[evaluation harness](../development/evaluation-harness.md).

The identifiers recorded inside the freeze records
(`kind: "commitrift.frozen-evaluation"`, paths "relative to
fixtures/phaseNN", the label files' `$comment`) are frozen content and are
not rewritten; so are the files they hash — the frozen applications keep
their `commitrift-contracts.json` (`kind: "commitrift.contracts"`) under
that name. Every file of a frozen set stays byte-identical. Read a `FROZEN.json` path as relative to its own set
directory (`fixtures/eval/pNN`).

## The sets

| set | application | cases (+ / − / unsupported) | author, and what they had read | frozen |
| --- | --- | --- | --- | --- |
| [`p11`](../../fixtures/eval/p11) first-party | `fixtures/apps/commerce-app` | 21 (8 / 10 / 3) | the implementer, with the fixture, **before the engine existed**; the fixture was designed with the detector families in mind | labels only (no `FROZEN.json`); one dated correction |
| [`p11/heldout`](../../fixtures/eval/p11/heldout) | `fixtures/apps/shop-app`, `fixtures/apps/team-app` | 12 (shop 10, team 2) | labeled from the verdicts of their earlier hand-written scenarios; contracts transcribe only what those scenarios declared | labels only |
| [`p12`](../../fixtures/eval/p12) | `eval-app`: 15 entries, a three-module schema behind a barrel, object and class repositories | 15 (7 / 6 / 2) | the implementer, after implementing and before discovery ever ran on it | checksums recorded before the first scoring (archived record, see [history](../HISTORY.md)); no `FROZEN.json` in the tree |
| [`p13`](../../fixtures/eval/p13) | `eval-app`: 16 tables in five modules behind a barrel, a `withReplicas` replica set, repositories, a `withAuth` route wrapper, zod-validated inputs, a `withRetry` wrapper, an at-least-once worker | 27 (15 / 11 / 1) | an AI agent apart from the implementation: read earlier documents and fixtures, not the in-progress Rust changes | `FROZEN.json`, 29 files, 2026-10-27 |
| [`p14`](../../fixtures/eval/p14) | `eval-app`: 28 tables in five modules, replica set, Server Actions and route handlers, `withRetry`, zod 3 | 38 (18 / 18 / 2) in 12 construct families, each with a vulnerable and a safe counterpart | an AI agent that read no `crates/` or current design notes and ran no Midwrite command | `FROZEN.json`, 39 files, manifest digest |
| [`p15`](../../fixtures/eval/p15) | `eval-app`: input-domain and initial-state sensitivity; 14 service modules | 30 (13 / 14 / 3); every positive violated only for some permitted inputs or states, with a safe `counterpart` and an `inputSensitivity` field | as p14; read only the user-facing docs | `FROZEN.json`, 26 files, manifest digest |
| [`p16`](../../fixtures/eval/p16) | `eval-app`: realistic surfaces around a write — zod 4, retry loops and SQLSTATE retries, request bodies, held predicates, multi-row locks, `NULLS NOT DISTINCT`, generated ids, two replica sets; 41 tables in six modules | 40 (18 / 19 / 3); every decided property is **declared** in the set's contracts | an AI agent that read no `crates/`, `runtime/` or design notes and ran no build or Midwrite command | `FROZEN.json`, 38 files (including its `EVALUATION-DATASET.md`), manifest digest |

The p14, p15 and p16 directories carry their own `EVALUATION-DATASET.md`
(format, families, every case, caveats). The p13 description follows; p11
and p12 are described by their label files' `$comment`.

### p13 in detail

A plain TypeScript application without `node_modules`, type-checked once
against the Drizzle 0.45.4 typings with a zod stub (in a scratch copy, not
part of the fixture). Labels come from PostgreSQL semantics, READ COMMITTED
unless a case says otherwise. Label format: the p12 format (`entry`,
`class`, `label`, `effectful`, `tables`, `property`, `environment`, `why`)
plus `id` (E01–E27; one entry can be labeled in two environments),
`pairedWith` (the label concerns the entry running concurrently with
another entry), `observable` (the property is about what callers are told —
return value or response — not table contents; in some cases the table
ends in a legal-looking state and only the return values reveal the
violation) and `replicas` (the routing of the replica handle the label
depends on: `select` reads the replica; `insert`, `update`, `transaction`
and `$primary` go to the primary, an asynchronous standby that may lag).
`expectedSchema` lists the tables, generated keys, 12 foreign keys (all
`NO ACTION`, not deferrable), unique keys and the replica handle.

Classes are the label author's names, not implementation identifiers:
check-then-act 9, status-transition 4, stale-read 4, lock-cycle 2,
resubmitted-write 2, ambiguous-commit 2, duplicate-processing 2,
read-modify-write 1, unknown 1. Its contracts use only `retry: on-crash`,
`delivery: at-least-once` and four `atMostOne` rules; properties outside
that vocabulary (deadlock freedom, return values, read-your-writes) are
stated in the labels only, and a scorer that needs them declared must add
them in a separate post-freeze file and say so.

Caveats a scorer must keep: two negatives are negative only at READ
COMMITTED (at REPEATABLE READ one duplicates and one fails with 40001); a
"no orphan" negative still has one call fail with 23503 and a "one payment"
negative still returns false to a retried call — those other properties are
not labeled; the replica cases assume a lagging replica (with a synchronous
`remote_apply` standby they would be negative). The PostgreSQL lock matrix
run after the freeze agreed with every label it bears on.

## Freezing rule

* An application and its labels are written, then **frozen before any
  scoring** and before any Midwrite command runs on the application:
  `FROZEN.json` records the SHA-256 of every application file and of the
  labels, and a manifest digest — the SHA-256 of the lines
  `<sha256>  <path>\n` sorted by path in byte order (the output of
  `shasum -a 256` over the sorted paths).
* The scorer (`scripts/eval-sets.py`) verifies the freeze before every
  run: for p13 every listed file must keep its hash; for p14–p16 also no
  file may be missing or added under `eval-app/`, and the manifest digest
  must match. p11 and p12 have no `FROZEN.json`; the label file's hash is
  recorded in each output instead.
* **Labels are never edited.** A disagreement goes into the label file's
  `corrections` array, dated, with a reason, and the label stays (example:
  p11's checkout route, labeled unsupported because `request.json()` was
  not modeled when it was written; the correction records that it now is).
* A set's **first scoring** is recorded once (`--first` refuses to
  overwrite it) with the binary's digest beside it. Before it, only
  aggregates of the previous binary's run are looked at, never per-case
  results. Scores after changes the set prompted are reported separately as
  post-feedback, never as held-out results. Once a set has prompted a
  change it is a regression set.

## Scoring

* **Model prediction.** An unpaired case is predicted positive when the
  entry has a candidate of its own (every actor is the entry) whose status
  is `MODEL_COUNTEREXAMPLE` and whose every counterexample is validated
  (replays in the model and violates the property). A paired case
  (`pairedWith`) needs a candidate whose actors are exactly the two entries.
* `TP`/`FP`/`FN`/`TN` compare the prediction with the frozen label. An
  `UNSUPPORTED`, `BLOCKED`, `INCONCLUSIVE` or `NO_VIOLATION_WITHIN_BOUNDS`
  result on a positive is a **false negative**: unsupported code is never
  scored as found or as safe.
* Cases **labeled** unsupported are outside every denominator; the scorer
  records whether the tool reported them unsupported or claimed them safe
  (`claimedSafe`).
* p14 and later also report scores without the cases whose property must
  be declared and is not (`excludingUndeclared`), and per family; p15 and
  p16 report whether each positive and its safe counterpart were both
  classified correctly.
* Stages are measured each on its own and never merged into one accuracy
  number: schema recognition, entry extraction, effect extraction (a bound
  database write), candidate generation (a candidate of the labeled class;
  negatives may have one — a candidate is a question the model answers),
  model-checkable candidates, counterexample validity, unsupported
  handling. **Static hazard prediction** (a candidate of the labeled class
  exists, unblocked and without a recognized protection) is reported next
  to model prediction for comparison. Original-code reproduction is not
  measured on these sets: the evaluation applications have no authorized
  harness and are never executed.
* Whether the application *intends* a heuristic property is not measured.

## Reviewing a set

For a future independent reviewer, each item self-contained:

1. Read a set's labels (`id`, `entry`, `class`, `label`, `property`,
   `environment`, `why`, and for later sets `family`, `isolation`,
   `counterpart`, `inputSensitivity`) and the `$comment` stating who wrote
   them and what they had read. Check each `why` against the source and
   PostgreSQL 15+ semantics.
2. `scripts/eval-sets.py SET --verify-only` checks the freeze; a changed
   file refuses to score.
3. Per-case evidence is in a scoring's `stages.model.cases` (prediction,
   static prediction, candidates and statuses).
4. Reproduce a counterexample: `midwrite discover fixtures/eval/p16/eval-app
   --model-check --emit-scenarios DIR`, then `midwrite model-check
   DIR/<file> --validate-postgres` (needs the PostgreSQL server binaries;
   nothing of the application is executed).
5. Record reviewer, date, case id, agree / disagree / unsure and the reason
   in `corrections` without editing a label, and report the agreement rate
   separately from the scores.

Questions most worth an independent look: whether the static false alarms
that model checking clears are safe for every input or only within the
explored worlds; whether a heuristic "no invocation fails" should ever be
reported for lock-order candidates (two opposite transfers deadlock and one
gets 40P01, which PostgreSQL confirms but a label written for transfers from
one wallet calls negative); whether every "read a row, then write keyed by
its id" status transition labeled positive is a hazard.

**No independent human review of any set has been performed.**

# Phase 7 final report

Question: *does the model being checked correspond to the program
developers wrote — and can a model counterexample be reproduced by that
program?*
Measured answer: **for a documented subset, yes.** CommitRift now extracts
the anonymous callback of a wrapped Server Action as written — including
the original saas-starter `inviteTeamMember` — with joins, projections,
generated ids, defaults, `RETURNING`, exceptions and `switch`. It runs a
model counterexample against the **original TypeScript code** of a
first-party application, with Drizzle sending its own SQL to a disposable
PostgreSQL in the counterexample's order. Two counterexamples were
reproduced this way (5 of 5 runs each), including one with a lock wait
PostgreSQL reported exactly where the model predicted it. Compact path
summaries make deep call-chain analysis 32–55 % cheaper in path analysis
with byte-identical results. Ordinary `check`/`trace` output is unchanged.

Measured on 2026-10-26 on one Apple Silicon Mac (macOS 27.0.1, PostgreSQL
17.11, Node.js 22.23.2, Drizzle 0.45.4) and in a Debian 12 container on the
same machine (Linux 7.0.12 aarch64, PostgreSQL 15.19, bubblewrap). Raw data:
`reports/phase7/`.

## 1. Architecture

| Area | Change | Document |
| --- | --- | --- |
| Function identity | functions identified by module + start position, shared with the static analyzer's naming; source selectors for callbacks, returned functions, methods, object properties, anchors | [`SOURCE-IDENTITY.md`](SOURCE-IDENTITY.md) |
| Callbacks and wrappers | bounded analysis of how a wrapper invokes a callback (returned/awaited/fire-and-forget/during wrap/stored/never), guards and arguments; declared contracts required when source facts are conditional or unknown | [`CALLBACK-RESOLUTION.md`](CALLBACK-RESOLUTION.md) |
| Compact summaries | per inlined call scope, exit-grouped trace fragments spliced onto incoming traces; exact when no fact variable is shared; default | [`COMPACT-SUMMARIES.md`](COMPACT-SUMMARIES.md) |
| Control flow | `try`/`catch`/`finally` (engine handlers; crashes skip them), `switch`, `??`, value-position `\|\|`/`&&`, object literals, `for…of` over array literals, destructured parameters | [`CONTROL-FLOW.md`](CONTROL-FLOW.md) |
| Database extraction | inner/left joins, projections, row arrays (`length`, `[0]`), `INSERT … RETURNING`, sequences (non-transactional) and constant defaults, `values(obj)`, enum members | [`DRIZZLE-EXTRACTION.md`](DRIZZLE-EXTRACTION.md) |
| Fidelity | extraction report (facts vs declared assumptions, semantic coverage), unknown expressions sealed into `Unknown` operations, source spans per operation | [`SOURCE-MODEL-FIDELITY.md`](SOURCE-MODEL-FIDELITY.md) |
| Runtime scheduler | Node workers (one per actor) with a coordinated `pg.Pool` and Drizzle call-site attribution; Rust coordinator with plan-ordered release and `pg_stat_activity` lock observation | [`RUNTIME-SCHEDULER.md`](RUNTIME-SCHEDULER.md) |
| Differential verification | plan from the counterexample, statement/cell/row/outcome comparison, invariant on observed rows, evidence level `original-code-reproduced`, CP7103 | [`DIFFERENTIAL-VERIFICATION.md`](DIFFERENTIAL-VERIFICATION.md) |
| Reports | model-check report schema v2 (superset; v1 kept when no Phase 7 section), HTML section | `docs/phase7/schemas/` |

Not done: shared semantic lowering (M2), cross-call-site parameterized
summaries, crash/retry against original code, schema facts from Drizzle
definitions, `postgres-js` coordination
([`IMPLEMENTATION-PLAN.md`](IMPLEMENTATION-PLAN.md)).

## 2. Correctness

| Gate | macOS arm64 (PostgreSQL 17.11) | Linux aarch64 container (PostgreSQL 15.19, bubblewrap) |
| --- | --- | --- |
| `cargo fmt --all -- --check` | clean | failed: test code appended after formatting (fixed; re-checked on macOS) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean | clean (rustc 1.99.1) |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **412 passed, 0 failed, 0 ignored**, nothing skipped (381 Phase 1–6 + 31 Phase 7) | **411 passed, 0 failed, 0 ignored** (one test fewer: the HTML-escaping test was added after the container copied the sources) |
| `cargo build --release` | ok | ok; packaged binary analyzes the fixtures |
| Phase 4 Node/Drizzle replay | in the test suite | `drizzle-node` replay, `--repeat 5`, generated harness: exit 0 |
| Schema validation | 19 model-check reports (3 v2, 16 v1), 0 errors, corrupted copies rejected (built-in validator) | — |

The Linux wrapper's final status 3 comes from its packaging smoke step
(`check fixtures` exits 3 on the intentionally unparsable fixture), as in
Phases 4–6.

New tests (31): model `phase7.rs` (6: sequences, joins, nesting,
reduction, PostgreSQL differential), analyzer `identity` unit tests (4),
replay `source` unit tests (3), analyzer `phase7.rs` (6: selectors,
wrapper shapes, contracts, team-app extraction), CLI `phase7.rs` (12:
original-code reproduction, lock wait, mismatched input, authorization,
declared programs, compact-vs-legacy on every fixture, exceptions vs
crashes, helper handlers, unsupported exception handling, `switch`/loops,
original saas-starter extraction, HTML escaping). One Phase 6 test was
updated because Phase 7 deliberately models constructs it expected to be
unknown (`for…of` over a literal, `try`); it now asserts the constructs
that remain unknown.

**Problems found and fixed during Phase 7** (each covered by a test):

1. Unknown expressions in local operations failed the actor instead of
   reporting `UNSUPPORTED` (silent conversion of unsupported source into a
   modeled exception) — fixed by sealing them at load.
2. Value-position `a || b` / `a && b` evaluated to a boolean.
3. A caught failure resumed at a local operation without settling the
   thread (stuck state) — found by the control-flow tests.
4. Drizzle copies `then` into select builders with a mixin, so patching
   `QueryPromise.prototype.then` attributed no SELECT to source — found by
   the first original-code run, which then *diverged* (the wrapper's own
   `getUser()` query was matched by kind/table); fixed by instrumenting
   every prototype of each builder chain.
5. Statement attribution needs the statement's whole span (the call
   position is the last builder method's line) — `SourceRef.end_line`.

No disagreement between the model and PostgreSQL (102 new schedules, both
versions) or between the model and the original code (team-app scenarios)
remained.

## 3. Source fidelity

| | team-app `inviteTeamMember` | team-app `acceptInvitation` | saas-starter original `inviteTeamMember` |
| --- | --- | --- | --- |
| entry | callback of `validatedActionWithUser` | same wrapper, Drizzle transaction | callback at `app/(login)/actions.ts@401:3` |
| represented / assumed / unsupported | 5 / 4 / 0 | 5 / 4 / 0 | 5 / 4 / 0 |
| assumptions | invocation awaited; authenticated user; validation succeeded; key order without `orderBy`; sequence start; unmodeled timestamps | same | same |
| unexpected runtime effects | none; 2 wrapper reads outside the model (declared) | none; wrapper reads outside the model | not executed |
| evidence | `original-code-reproduced` (5/5) | `original-code-reproduced` (5/5) | `postgres-confirmed-schedule` (model SQL) |

Models that could not be verified against original code: the original
saas-starter application (not authorized to execute; it also uses
`postgres-js`, for which there is no coordinator adapter). Unsupported
constructs are listed in [`LIMITATIONS.md`](LIMITATIONS.md).

## 4. Performance

[`PERFORMANCE.md`](PERFORMANCE.md):

* Path analysis −32 % to −55 % (midday, school-ledger, depths 3–8),
  identical findings; peak RSS −0.3 to −5 MiB.
* `commitrift check` on midday: 0.230 s at depth 3 (unchanged), 0.230 s at depth 6 (Phase 6: 0.260 s); peak RSS 91.1 MiB (Phase 6: 97.7) and 99.2 MiB at depth 6 (Phase 6: 112.8).
* Model checking: Phase 6 workloads unchanged (sub-millisecond); extracted
  team-app action at 4 actors 145 states / 6 ms (first counterexample);
  exhaustive 4-actor mitigation 1 984 states / 83 ms.
* Original-code verification ≈ 1.9 s per run vs ≈ 1.1 s for model-SQL
  replay.

## 5. Real-world evaluation

* Original source analyzed: saas-starter `inviteTeamMember` (rev 6e33e58),
  extracted as written (Phase 6 needed an adapted rewrite).
* Counterexample: two concurrent submits create two pending invitations
  for the same email (10 transitions); PostgreSQL confirms the model's
  schedule.
* Original-code reproduction: **none for saas-starter** (not executed).
* First-party reproduction: the same structural pattern in
  `fixtures/phase7/team-app` reproduced on original code, 5/5 — evidence
  about the pattern, not about saas-starter.
* Human-reviewed findings: none. Maintainers were not contacted.
* Remaining gaps: whether concurrent submits occur in practice; the
  wrapper's preconditions; `postgres-js`; the unique constraint the schema
  lacks (a hypothetical `UNIQUE(team_id, email)` removes every
  counterexample within bounds at 2–4 actors, model only).

## 6. Definition of done

| # | Requirement | Status |
| --- | --- | --- |
| 1 | analyzer and model checker remain functional | yes (all Phase 1–6 tests; corpus outputs byte-identical) |
| 2 | anonymous callbacks and selected wrappers resolved | yes |
| 3 | first-party app with real-world wrapper structure model-checked without rewriting | yes (`team-app`) |
| 4 | compact summaries preserve failure-path behavior | yes — exactly, on 22 022 corpus function analyses × 3 depths and all fixtures |
| 5 | deep call chains measurably cheaper or limits quantified | yes (−32 to −55 % path analysis); summary construction now dominates and is quantified |
| 6 | JOIN, RETURNING, generated values supported and validated | yes (PostgreSQL differential, real Drizzle SQL) |
| 7 | mapping records assumptions and unsupported operations | yes (extraction report) |
| 8 | counterexample from an eligible original TypeScript function | yes |
| 9 | scheduler executes it against disposable PostgreSQL | yes |
| 10 | modeled vs observed SQL compared | yes, per statement and cell |
| 11 | a counterexample reproduced through original code in an authorized first-party app | yes (two) |
| 12 | unsupported source never disappears | yes (sealing fix; refusals; divergence reporting) |
| 13 | POR correctness checks valid | yes (existing suites + new reduction tests) |
| 14 | existing PostgreSQL differential tests pass | yes (17.11 and 15.19) |
| 15 | performance and memory measured | yes |
| 16 | workflow documented and reproducible | yes (`docs/phase7/`, scripts, fixtures) |

Milestone 11 (state-space optimization) was measured, not optimized; the
original external application was not executed, so no external finding is
claimed as reproduced.

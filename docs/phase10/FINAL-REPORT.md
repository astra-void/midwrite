# Phase 10 final report

Phase 10 explains why the Phase 9 DPOR was slow, replaces it with a
source-set DPOR that combines sleep sets with a summarizing state cache,
tests every reduction against the unreduced search with a coverage-driven
generator, mutation testing of the dependence relation and a state-by-state
audit, and runs live-process connection failures and postgres-js in-flight
faults against original first-party code. Every number below comes from a file
under `reports/phase10/`; nothing is estimated unless it says so.

**Snapshot.** Base commit `81d27e4` (Phase 9 finalization, clean tree at the
start) plus the uncommitted Phase 10 changes. Source digest
(`scripts/phase8-digest.py`, 593 files; docs and reports excluded)
**`59275b50edea91f11ac8ee9e5622e8630338ffc12c3e4a2d0f5b6b2cca1817c7`** on macOS
and in the Linux container; release binary sha256 `ec9c019f0a01ad1b…`
(`reports/phase10/final/`). The benchmark, the runtime evidence and the
Phase 8/9 regressions ran at digest `dac5dae3…`, which differs from the final
one only in `scripts/phase10-report-check.py` (the Python report checker,
which then checked their reports); the release binary is the same file. Nothing was
committed or pushed. macOS: Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2,
PostgreSQL 17.11 (Homebrew), runs confined by `sandbox-exec` (IP networking
denied).

## 1. Why the classic DPOR was slow (profile)

`BASELINE.md`. On the four-actor team-app workload the classic search ran
984 511 maximal executions containing **148 distinct Mazurkiewicz traces**
(each explored ≈ 6 650 times) and 32 distinct terminal states; 99.96 % of its
3 476 614 transitions ran below a state an earlier frame had already opened;
no race fell back to "every enabled transition". The cause is duplicate
exploration (no sleep sets) and no state caching — not over-conservative
dependencies.

## 2. Algorithm

* **Chosen**: source-set DPOR (Abdulla et al.) with sleep sets, plus a state
  cache whose entries carry an interned summary of the `(transition,
  footprint)` events below them; on a cache hit the summary's races are
  replayed against the current trace (Yang et al.). Each mechanism can be
  switched on separately; the three configurations are selectable
  (`--source-dpor stateful-sleep|stateful|stateless-sleep`). Optimal DPOR was
  not chosen (`SOURCE-DPOR.md`).
* **Dependency semantics**: the Phase 9 footprint relation, unchanged
  (`DEPENDENCY-SEMANTICS.md`); audited state by state.
* **Enabling relation**: the event after which a transition became enabled is
  *dependent* with it, whatever the footprints say — never an ordered
  predecessor, because enabling can be disjunctive (`ENABLING-RELATIONS.md`).
* **Backtracking**: every racing event (not only the last); one initial of
  `notdep(e).u` unless the backtrack set already holds one; sleeping members
  of a choice class make their siblings obligations; disabled transitions are
  alternatives.
* **State-cache integration**: the exact canonical key is the identity;
  summary, explored step count and the explored sleep set are metadata beside
  the key; a cached exploration is reused only if its sleep set is contained in
  the visit's and it was explored at no more steps.
* **Cycles**: only completed states are reused; a state on the stack is
  explored again, so a cycle is followed to `maxSteps` (as before). No
  liveness claim.
* **Property preservation**: terminal states (modulo cross-channel effect
  order), final invariants, counterexamples, stuck states, deadlocks, enabled
  unsupported behavior, step truncation. Refused: `always` invariants and
  `maxContextSwitches` (`POR-CORRECTNESS.md` §4).

## 3. Correctness

### Quality gates (final digest `59275b50…`)

| gate | macOS arm64 | Linux aarch64 container |
| --- | --- | --- |
| `cargo fmt --all -- --check` | exit 0 | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | exit 0: **555 passed, 0 failed, 2 ignored** (21.2 min) | exit 0: **555 passed, 0 failed, 2 ignored** |
| `cargo build --release` | exit 0 | exit 0 |

Phase 9 ended at 524 passed, 0 ignored. The 31 new tests: `sdpor.rs` 3,
`por_adversarial.rs` 3, `por_audit.rs` 2, `por_mutation.rs` 2,
`por_regressions.rs` 3, CLI `phase10.rs` 13, `proxy_postgres.rs` 4, proxy
unit 1. The 2 ignored are tools, not skipped checks: `regenerate_fixtures`
(rewrites the regression fixtures) and `dump_seed` (prints a generated
scenario). One Phase 9 test was changed on purpose:
`a_lost_acknowledgment_with_a_live_process_is_refused_not_faked` asserted the
Phase 9 refusal; it is now
`a_lost_acknowledgment_with_a_live_process_is_a_broken_connection_not_a_kill`
and asserts that the run **diverges** (the Phase 9 harness has no client error
handler, so the process dies) with the driver's unhandled `error` event
quoted. The analyzer's single-file fixture test skips the Phase 10
applications, as it skips those of Phases 7–9.

An earlier full run at the same code (before two test-side fixes) had 553
passed and 2 failed: the analyzer walker reading the Phase 10 fixtures, and a
test that inserted `null` into a report it then parsed
(`reports/phase10/final/first-run/`). Two further clean macOS runs (555 / 0 / 2
each) preceded a comment-only fixture change (`pre-comment-fix/`) and the
report-checker change (`at-dac5dae3/`); the numbers above are from the final
digest.

In debug builds the adversarial differential campaign runs 120 seeds and the
mutation tables use 25 random / 90 generated seeds (release: 360, 60, 270);
the evidence runs below are release builds with larger corpora.

### Differential evidence

| suite | scenarios | result |
| --- | ---: | --- |
| targeted Phase 6–9 workloads | 14 | every search agrees with the unreduced search |
| Phase 9 model scenarios + pool/crash/in-flight workloads | 10 | agree |
| Phase 9 random generator (seeds 1–150 + regression seeds) | 149 compared, 1 unverified | agree |
| adversarial generator, release, 360 seeds | 360 | agree |
| adversarial generator, release, 3 000 seeds (`reports/phase10/evidence/adversarial-3000-run1.txt`) | 3 000 compared, 0 unverified | agree (sleep sets, classic DPOR, three source-DPOR configurations) |
| metamorphic transformations | 208 cases | verdict, terminal-state count and stuck-state existence preserved |
| regression fixtures (both directions) | 7 | handled by every algorithm; each still exposes its defect |
| dependence audit | 144 workloads; 84 077 independent pairs, 31 286 enabling pairs | 0 violations, 0 footprint drift; the claim-only mutation is detected on 11 workloads |

### Defects found and fixed

| defect | where | how found | regression fixture |
| --- | --- | --- | --- |
| a sleeping member of a choice class hid its siblings | source-DPOR (development) | old random seed 87 | `sleeping-choice-member-random-{87,234}` |
| the enabler as an ordered predecessor | source-DPOR (development) | shared-process workloads; mutation test | `enabler-orders-the-enabled-adversarial-{30,53}` |
| only the last race handled | **classic DPOR of Phase 9** (and source-DPOR at the time) | adversarial seeds 165, 75, 245 (9 transitions: two actors in one process, one crash, no pool) | `last-race-only-adversarial-{75,165}` |
| the Phase 9 pool-claim defect | Phase 9 search configuration | reproduced by the claim-only mutation | `pool-claim-without-pool-access-phase9-workload` |

The oracle itself was corrected once: terminal states are compared modulo the
interleaving of effects on different channels (adversarial seed 6); the
unreduced search distinguished an order no invariant can observe.

### Generator coverage

`ADVERSARIAL-GENERATION.md`: nine profiles, 14 required feature combinations
(pool release with a waiting claimant, crash with a pending COMMIT, retry with
a unique constraint, lock release resuming a statement, ServerComplete after
a restart, both lost-acknowledgment outcomes, shared process crash without a
pool, tasks of one actor, application variables in a shared process,
sequences, skip-locked, a reachable deadlock, serializable predicate reads):
each covered by 125–538 of 3 000 generated models.

### Mutation adequacy

Deleting the `Pred`, `RowWrite`, `Global`, `Actor`, `App`, `LockChange`,
`Ser`, `Snapshot`, `CommitWrite` or `Ext` access is noticed by some workload;
deleting `RowRead`, `LockCheck`, `Wait`, `Seq` or `Budget` by none (analyzed
as redundant, not proved). The claim-only mutation (the Phase 9 defect) is
noticed for the Phase 9 search configuration and not for any Phase 10 search.

### Remaining proof obligations

`POR-CORRECTNESS.md` §6: the summary argument; source sets with sleep sets
under conditional enabling and non-deterministic choices; generalization of
the audited relation beyond the audited bounds; completeness of the canonical
state.

## 4. Performance

`STATE-SPACE-PERFORMANCE.md`, raw `reports/phase10/final/por-bench.md`, run
with the final release binary (sha256 `ec9c019f…`).

**Phase 9 → Phase 10, primary workload (team-app, 4 actors).** Classic DPOR:
3 476 614 transitions, 30 362 ms (Phase 9) / 37 973 ms (Phase 10, every race).
Sleep sets: 2 291, 44 ms (unchanged). **Source-DPOR stateful+sleep: 574
transitions, 12 ms** — 6 057× fewer transitions than the classic search, 4.0×
fewer than sleep sets; same 32 terminal states.

| workload | sleep sets | classic DPOR | sdpor stateful+sleep |
| --- | ---: | ---: | ---: |
| independent actors (5) | 3 124 | 20 | 20 |
| mostly independent (4) | 1 416 | 14 394 505 | **106** |
| conflicting, safe (4) | **4 638** (26 ms) | 17 667 614 | 4 843 (55 ms) |
| pool contention | **204** | 3 944 | 210 |
| write skew (SERIALIZABLE) | 423 | 25 586 | **357** |
| crash/retry (4) | 24 024 | truncated (120 s) | **10 584** |
| team-app 5 | 18 042 (398 ms) | truncated (120 s) | **3 849 (93 ms)** |
| team-app 7 | 1 084 010 (30.8 s, 117.9 MiB) | not run | **190 830 (6.0 s, 32.7 MiB)** |

(Transitions executed; full matrix of 13 workloads × 6 modes in the raw file.)
Where it loses: highly conflicting safe updates (2.1× slower than sleep
sets), pool contention (3 ms vs 1 ms), and without the state cache
(stateless+sleep) on most dependent workloads. Peak RSS is within +0.6 MiB of
sleep sets on small models and 3.6× lower at 7 actors. Time to the first
counterexample is 0 ms in every mode on the measured counterexample
workloads. **Sleep sets remain the default** (`STATE-SPACE-PERFORMANCE.md`
§Decision); `--source-dpor` is opt-in.

## 5. Runtime evidence

`RUNTIME-VERIFICATION.md`, `LIVE-CONNECTION-FAILURES.md`,
`POSTGRES-JS-IN-FLIGHT.md`; reports and `SUMMARY.md` under
`reports/phase10/evidence/runtime-final/` (digest `dac5dae3…`, the final
release binary, 5 runs each).

| | |
| --- | --- |
| scenarios × runs | 22 × 5 = 110 original-code runs (first-party fixtures only) |
| drivers | node-postgres 8.23.1 (13 scenarios), postgres-js 3.4.5 (9) |
| faults | 19 scenarios break a connection under a live process, 3 kill a postgres-js process with an operation in flight |
| outcome | **21 of 22 scenarios reproduce (counterexample) or recover (probe) in 5 / 5 runs**; the no-error-handler variant **diverges** in 5 / 5, as expected |
| fates realized and observed | answered-and-withheld (server `CommandComplete COMMIT` / `INSERT 0 1` seen on the wire, durable), not-reached (backend `idle in transaction`, then rolled back), held-for-later-delivery (session kept, COMMIT delivered after the application's next step) |
| driver errors (quoted) | node-postgres `Connection terminated unexpectedly`; postgres-js `write CONNECTION_CLOSED <replay-root>/px/.s.PGSQL.5432` |
| worker | alive afterwards in every agreeing loss (operating system); the retry used a new backend (node-postgres) |
| process-crash in-flight on postgres-js (new) | 3 scenarios, 5 / 5 each |
| isolation | READ COMMITTED (19), REPEATABLE READ (1), SERIALIZABLE (2) |
| PostgreSQL | 17.11 (macOS); 15 (Linux, §6) |
| report check | 22 reports valid; 517 corruptions applied, all rejected |

Findings about the drivers, not about the model: an unhandled
`error` event on a checked-out node-postgres client ends the Node process
(the application's recovery code never runs); postgres-js reads `pg_type` on
every new connection (a statement fault must not target it); postgres-js's
`sql.end()` did not return through the proxy (cause not established).

## 6. Compatibility

| | result |
| --- | --- |
| static analysis | `commitrift-core`, `-analyzer`, `-corpus` sources unchanged; **144 / 144** corpus outputs byte-identical to the Phase 9 binary (4 projects × depths 3, 6, 12 × 3 path modes × json, sarif, text, trace) |
| model checking | **56 / 56** Phase 6–9 scenarios identical to the Phase 9 binary, JSON and text, no exempt field (default search) |
| classic DPOR (`--dpor`) | changed on purpose (every race): it may execute more transitions and finds terminal states the Phase 9 search missed; not part of the identity check |
| original-code regressions | Phase 9: 11 scenarios × 3 runs, Phase 8: 21 scenarios × 3 runs, all followed and matched, with the final binary |
| report versions | schemas 1–4 unchanged; schema 5 only with source-DPOR or a connection loss; the 32 regression reports stay 3/4; Phase 10 crash-only postgres-js reports stay 4 |
| report check | 22 runtime + 4 source-DPOR reports: valid, 562 corruptions rejected; 32 regression reports: valid, 299 rejected |
| HTML | connection losses rendered (observed/inferred lanes); an escaping test poisons every string of a fresh loss report |
| cross-platform | **matching snapshot**: the container's copy has digest `59275b50…` (593 files), equal to the macOS tree at which the macOS gates ran. Debian 12 container on Docker Desktop (linuxkit 7.0.12, aarch64), rustc 1.99.1, Node 22.23.4, PostgreSQL 15.19, bubblewrap 0.8.0, non-root user. Gates as above; Phase 9 evidence (10 scenarios × 3) followed and matched, its report check 11 valid / 194 corruptions rejected; **Phase 10 evidence 22 scenarios × 3 runs: 21 reproduce or recover in 3 / 3, the no-error-handler variant diverges in 3 / 3**, with the same driver errors as on macOS (`reports/phase10/linux/`). Infrastructure: the first attempt failed because the Docker daemon was not running (`linux-run-docker-not-running.log`); Docker Desktop was started and the run repeated. Not compared across platforms: timings, the benchmark, the corpus identity (private projects are not in the container), the Phase 8 regression (macOS only). |

## 7. Limitations

`LIMITATIONS.md`. In short: DPORs are evidence-checked, not proved; sleep
sets stay the default; only a closed socket is realized as a connection
failure, only for a COMMIT or an autocommit write; postgres-js 3.4.5 only;
all original-code evidence is from first-party fixtures; no external
application was executed and no finding was independently reviewed.

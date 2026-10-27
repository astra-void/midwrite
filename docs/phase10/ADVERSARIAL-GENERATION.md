# Adversarial generation, mutation adequacy, minimization

## Why the Phase 9 generator was not enough

`tests/common/scenarios_gen.rs` draws two or three actors on one table, with
no pools, shared processes, tasks, sequences or application variables. 700
passing seeds said nothing about the transitions it cannot express, and the
pool hand-over defect lived exactly there. Phase 10 adds a generator built
around one observation: **a dependency only matters where independence would
otherwise hold.** A scenario in which every actor touches one row tests
almost nothing, because everything is dependent anyway.

## The generator (`tests/common/adversarial.rs`)

A seed picks one of nine *profiles*; a profile fixes which resources actors
share and keeps everything else private (one table per actor, one external
channel per actor), so the dependency the profile exercises is not masked.

| profile | shared resource | other features |
| --- | --- | --- |
| Rows | one table; row, predicate and unique-key conflicts | locks, deletes, crashes, retries, SERIALIZABLE |
| Pool | a bounded pool of one process (1–2 connections) | tasks, crashes, lost acknowledgments, retries |
| Tasks | the locals and transaction of one actor (`spawn`/`join`) | read-modify-write |
| App | in-process variables of one process | crashes, retries, private externals |
| Seq | a sequence (autocommit inserts) | |
| Faults | the fault budget only (private data) | crashes, retries, lost acknowledgments, unique keys |
| Locks | rows locked `FOR UPDATE [SKIP LOCKED]`; two actors locking two rows in opposite orders | private externals |
| Serializable | predicate reads under SERIALIZABLE | inserts |
| Mixed | several of the above at once | |

Complexity is bounded explicitly: 1–3 actors, 1–3 statements each, at most one
crash, one retry, one lost acknowledgment, pools of 1–2, `maxSteps` 60.

### Coverage: generated features and observed features

Generated features (`profile.*`, `pool`, `unique`, `lock.skip-locked`, ...) say
what is in a scenario. *Observed* features say what its state space contains,
from a walk of up to 20 000 states (`observe`): transition kinds, a waiting
claimant that gets a connection by a release, a statement that resumes after
a lock wait, a crash with an operation pending on the server, a server
completion after a restart, concurrent transactions, deduplicated effects.
`COMBINATIONS` names 14 combinations a campaign must reach; the test fails if
one is covered by fewer than three models per 360 seeds.

3 000-seed campaign (release, `reports/phase10/evidence/adversarial-3000-run1.txt`):
3 000 generated, 0 invalid, 0 unverified, 3 000 compared, 333–334 per profile.

| combination | models |
| --- | ---: |
| pool release + waiting claimant | 330 |
| pool + crash | 183 |
| crash + pending COMMIT | 242 |
| retry + unique constraint | 204 |
| lock release + statement resumes | 502 |
| ServerComplete + restart | 500 |
| lost acknowledgment (both outcomes) | 538 |
| shared process + crash (no pool) | 425 |
| tasks of one actor | 125 |
| app variable + shared process | 345 |
| sequence inserts | 380 |
| skip locked | 154 |
| deadlock reachable | 148 |
| serializable predicate reads | 236 |

The first coverage run found *deadlock reachable* at 0: no generated program
locked two rows in opposite orders. The Locks profile now does (45 % of its
seeds).

## Differential oracle

For every generated scenario the unreduced search runs first (the oracle);
if it hits a limit the seed is **unverified**, not passing. Then the sleep-set
search, the classic DPOR (on models whose unreduced search executes ≤ 3 000
transitions) and the three source-DPOR configurations are compared with it on
what a reduction must preserve (`common/por_diff.rs::discrepancy`): terminal
states modulo cross-channel effect order, verdict, counterexample existence
and validity (replayed on the model), stuck states, unsupported behavior,
truncation by the step bound.

What the campaigns found, in order:

| finding | algorithms | resolution |
| --- | --- | --- |
| seed 6 and others: two externals on different channels | **all** reductions vs the unreduced search | not a reduction bug: the oracle distinguished the global order of independent effects; terminal states are now compared modulo that order (`FORMAL-ASSUMPTIONS.md`) |
| seed 165 (two actors in one process, one crash, no pool): a terminal state missing | **classic DPOR of Phase 9**, every source-DPOR configuration at the time | every race is handled, not only the last (`DEFECT_LAST_RACE_ONLY`) |
| seed 75 | classic, stateless+sleep, stateful+sleep | same fix |
| seed 245 | stateless+sleep | same fix |

After the fixes: 360 seeds and 3 000 seeds, **0 discrepancies** for the
sleep-set search, the fixed classic DPOR and all source-DPOR configurations.

Earlier, during development (the old random generator and hand-written
workloads): random seed 87 (a sleeping choice member) and shared processes
without a pool (the enabler-as-ordered-predecessor rule). See
`ENABLING-RELATIONS.md`.

## Minimization and regression fixtures

`common/minimize.rs` shrinks a scenario greedily while a property holds:
fewer actors, fewer lines (single lines and blocks of 2–6), no faults or
retry policies, no pool or shared process, fewer rows and tables, no
application variables, plainer isolation, a smaller step bound. A candidate
that no longer loads is rejected.

Each defect above is kept as a fixture under
`crates/commitrift-model/tests/data/regressions/`, with its origin (generator
and seed), the original scenario and the minimized one. Each fixture is
checked two ways (`tests/por_regressions.rs`):

1. every algorithm agrees with the unreduced search on it, and
2. with the historical defect switched back on (`SdporConfig::defects`, or the
   claim-pool footprint mutation for the Phase 9 defect) the recorded
   algorithm still disagrees. A fixture that stopped exposing its defect
   would pass for the wrong reason.

| fixture | defect | origin |
| --- | --- | --- |
| `sleeping-choice-member-random-{87,234}` | a sleeping member of a choice class did not make its siblings obligations | random generator |
| `enabler-orders-the-enabled-adversarial-{30,53}` | the enabler was an ordered predecessor | adversarial |
| `last-race-only-adversarial-{75,165}` | only the last race handled | adversarial |
| `pool-claim-without-pool-access-phase9-workload` | the Phase 9 pool hand-over defect (the Phase 9 search configuration: last race only, no enabling rule) | the workload that exposed it in Phase 9 |

`regenerate_fixtures` (ignored) rewrites them.

## Mutation adequacy (`tests/por_mutation.rs`)

A test-only feature (`mutation-testing`) deletes one kind of access from every
footprint. A corpus "kills" a mutant if some workload then makes a reduction
lose a terminal state, a verdict or a stuck state. The two corpora:

| deleted access | old corpus (targeted + Phase 9 + 60 random) | generated corpus (270 seeds) |
| --- | --- | --- |
| `Pred`, `RowWrite`, `Global` | killed | killed |
| `Snapshot`, `CommitWrite`, `Ext` | killed (1–3 workloads) | not killed |
| `Ser` | killed (1) | killed (1) |
| `Actor` | not killed | killed (up to 15) |
| `App` | not killed | **killed (9–16)** |
| `LockChange` | not killed | **killed (6, by the DPORs)** |
| `Pool` | killed by the Phase 9 search configuration only | not killed |
| `RowRead`, `LockCheck`, `Wait`, `Seq`, `Budget` | not killed | not killed |

The claim-only mutation (the Phase 9 defect exactly) is killed for the Phase 9
search configuration and for the no-enabling-rule ablation, and not for the
fixed classic search or any source-DPOR configuration.

**Kinds no workload kills**, and why (an analysis, not a proof):

* `Seq`: inserts into one table allocate row ids in order, so two inserts that
  draw from one sequence already conflict through `RowWrite` of the same new
  row id; sequences are per column, so no two tables share one.
* `RowRead`: every transition that re-reads a row in this engine also writes
  it or holds a predicate read of it.
* `LockCheck`: a `SKIP LOCKED` probe is followed by its own acquisition
  (`LockChange`), which conflicts with the holder's.
* `Wait`: blocking and deadlock-victim choice are also lock and row
  conflicts.
* `Budget`: taking the last in-flight fault disables the other's; the
  disabled-alternative rule and the unreduced exploration of sibling fault
  outcomes cover it.

These kinds stay in the relation (deleting a conservative access is never the
right response to "no test notices"); the audit (`audit.rs`) is what checks
them directly.

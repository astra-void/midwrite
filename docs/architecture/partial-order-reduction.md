# Partial-order reduction

`crates/midwrite-model/src/footprint.rs` (dependence relation),
`explore.rs` (unreduced search, sleep sets, classic DPOR),
`explore/sdpor.rs` (source-set DPOR), `audit.rs` (independence audit).
The transition system the reductions work on is described in
[model semantics](../concepts/model-semantics.md).

Every reduction is **evidence-checked, not proved**. This page states what
each search preserves, the assumptions it relies on, how those are checked,
and what remains a proof obligation.

## Searches

| search | selected by | status | report |
| --- | --- | --- | --- |
| unreduced DFS / BFS | `--no-por` (`--strategy bfs\|dfs`), `"por": false` | the **oracle** every reduction is compared with | |
| sleep sets with the state cache | default | **default** | `bounds.reduction` |
| classic stateless DPOR | `--dpor`, `"dpor": true` | opt-in | `reduction: "dpor"` |
| source-set DPOR with sleep sets and/or a summarizing state cache | `--source-dpor [stateful-sleep\|stateful\|stateless-sleep]`, `"sourceDpor": "…"` | opt-in, experimental | `reduction: "source-dpor"`, `sourceDporMode`, `exploration.reductionMetrics` |

A DPOR that is refused (an `always` invariant, `maxContextSwitches`) runs
sleep sets instead, and the report says why
(`bounds.partialOrderReductionDisabledBecause`).

```text
                   engine.rs: enabled(s), apply(s, t) → (s', footprint)
                                    │
      ┌───────────────┬─────────────┼───────────────┬──────────────────────┐
      ▼               ▼             ▼               ▼                      ▼
 unreduced DFS/BFS  sleep sets   classic DPOR    source-set DPOR        audit.rs
 (the oracle)       (default)    (every race)    (cache, sleep,         (independence,
                                                  source sets)           enabling pairs)
      │               │             │               │
      └──── state table: exact canonical keys (keytable.rs, interned fragments) ────┐
                                   per-node reduction metadata, NOT in the key:     │
                                   summary id, steps explored at, sleep set ────────┘
```

## Dependence: footprints

Every executed transition records the abstract resources it read and wrote
*as it actually executed in that state* (`footprint::Access`). Two
transitions are **dependent** when some pair of their accesses conflicts.
The relation is meant to be a superset of semantic dependence; unknown
aliasing is never independence.

| access | recorded by | conflicts with |
| --- | --- | --- |
| `Actor(a)` | every step of actor `a` | every transition of `a` (shared locals, one connection per transaction); program order is tracked per task |
| `Pred(table, predicate)` | every scan, the key predicate of a foreign-key check | a `RowWrite` of that table whose old **or** new image matches (an evaluation error counts as a match) |
| `RowRead(row)` | re-reading a newer version (EvalPlanQual), a foreign-key check, an upsert | `RowWrite` of the row |
| `RowWrite(row, old, new)` | writes; commits (publishing an image); aborts (removing one); the *intended* image of an insert or update even when it fails or waits | the same row, or any row sharing an equal unique-key value with old/new (two NULL keys are equal under `NULLS NOT DISTINCT`) |
| `LockCheck(row)` / `LockChange(row)` | lock tests / grants and releases (per row for multi-row locks) | lock changes of the row |
| `Snapshot` | taking a snapshot; any statement that ends up waiting (it keeps its snapshot); a replica read | `CommitWrite` |
| `CommitWrite` | a commit that wrote; replica progress (`Replicate`) | `Snapshot` |
| `Ser` | every statement, commit and abort of a SERIALIZABLE transaction | `Ser` |
| `Wait` | starting to wait, resuming, ending a transaction somebody waits for | `Wait` |
| `Pool(process)` | pool arrival, release, and the claim of a handed-over connection | `Pool` of the same process |
| `Seq(i)` | sequence allocation (not transactional) | `Seq(i)` |
| `Ext(channel)`, `App(process, var)` | external effects, in-process variables | the same channel / variable |
| `Budget` | taking an in-flight fault | `Budget` |
| `Global` | crashes, retries, deadlock victims, unsupported behavior, unknown evaluation, a blocked step until its `Resume` | everything |

`ServerComplete` records the submitting actor plus the rows it commits; a
lost acknowledgment records the actor plus `Budget`. Local values
(JavaScript values, generated identifiers, loop counters, caught-error
inspection) are actor-local and add no footprint.

Two rules found by testing are part of the relation: an `INSERT … ON
CONFLICT DO NOTHING` that hit a conflict still records its intended image
(it does not commute with an uncommitted `DELETE` of the conflicting row),
and a READ COMMITTED statement that waits records `Snapshot` (its position
relative to other commits becomes observable).

## Enabling and disabling

Two transitions can also be dependent because one changes whether the other
is enabled.

| transition | becomes enabled by |
| --- | --- |
| `Step` of a task waiting for a pool connection | the release that hands the connection over (several, with capacity > 1) |
| `Resume` of a blocked statement | the end of the transaction it waits for |
| `ServerComplete` | the lost acknowledgment or crash outcome that left the operation on the server |
| `Retry` | the failure or crash it reacts to |
| `Crash` of a process | the first visible operation of **any** running task of the process — disjunctive |
| `Deadlock` | the wait that closes a cycle |
| a child task's `Step` | its parent's `spawn` |

Rules every DPOR follows:

1. **An enabling pair is dependent.** The *enabler* of `u` (the last event
   after which `u` became and stayed enabled) is a happens-before edge and
   a race candidate with `u`, whatever the footprints say.
2. **It is never an ordered pair.** Treating the enabler as a necessary
   cause hides the race whose reversal runs the *other* enabling task first
   (disjunctive `Crash`).
3. **Disabled transitions become alternatives** at the state before the
   transition that disabled them: no later race can request them.
4. **Alternatives of one choice are explored together** — the server's
   answer, a lost acknowledgment that reaches the server or not, the
   outcomes of a crashing process's in-flight operations. They are
   alternatives of one task, not concurrent processes; a *sleeping* member
   makes its awake siblings obligations.
5. **Every race is handled, not only the last.** With conditional enabling
   the earlier race is the one that matters.
6. **A blocked step is dependent with everything** until its `Resume`.

## Sleep sets (default)

From a state *s* explored with sleep set *S*: skip transitions in *S* and
those already explored from *s*; for the next transition *t*, the child's
sleep set is every *u* in *S* ∪ (explored earlier from *s*) ∪ (explored
before *t* in this visit) that is independent of *t* in *s*. Each (state,
transition) pair is explored at most once, so the reduced search never
executes more transitions than the unreduced one. Successors of sleeping
transitions are still computed once (their footprints are needed): the
saving is the subtree below them.

With canonical-state caching, sleep sets visit **every reachable state** of
the unreduced search — they prune transitions, not states. That preserves
every state predicate: `always` invariants, `final` invariants, stuck
states. Reduction is disabled when `maxContextSwitches` is set (a commuted
ordering can need more switches).

Sleep sets are chosen as the default over persistent sets (which need a
static over-approximation of every actor's future accesses; the useful
independence here is row-level and value-dependent) and over the DPORs
(below), because their soundness argument is the textbook one, they
support `always` invariants, and they serve every scenario.

## Classic DPOR (`--dpor`)

Flanagan–Godefroid, stateless, no sleep sets: memory is the stack, not a
table of visited states; two interleavings that merely converge to the same
state are both explored. Happens-before over the trace is built from
dependent footprints, per-task program order and enabling pairs; for each
enabled transition, every unordered dependent event is a race, and the
reversal is requested at the state before it (or every enabled transition
there, when the racing one was not enabled). Stateful DPOR with a plain
visited cache is unsound (a revisited state hides backtrack points) and was
not used here; the source-set search below adds a summarizing cache instead.

## Source-set DPOR (`--source-dpor`)

Source sets (Abdulla et al., POPL 2014) for backtracking with sleep sets,
plus a stateful search with per-state summaries (Yang et al., SPIN 2008).
The three mechanisms (`cache`, `sleep`, source sets) can be switched
separately (`SdporConfig`); the differential tests run every configuration.

* **Source sets.** For a race `(e_i, u)`, the *initials* of `notdep(e_i).u`
  are the transitions that can start the reordering. If the backtrack set
  before `e_i` already holds an awake initial, nothing is added; otherwise
  one awake initial; if every initial sleeps, the run is covered by the
  exploration that put it to sleep; if none is enabled there, every awake
  enabled transition is an alternative.
* **State cache with summaries.** Each completed state stores the interned
  set of `(transition, footprint)` descriptors executed below it. A visit
  that reaches a completed state does not recurse: it runs the race
  detection of every summarized event against the current trace (adding
  backtrack points to the stack frames, Flanagan–Godefroid style) and
  unions the summary upward. It ignores happens-before chains through
  events below the cached state, so it finds at least the races the full
  continuation would.
* **Reuse conditions.** A completed exploration of a state is reused only
  when the state is completed (one still on the stack is explored again, so
  a cycle is followed to the step bound), it was explored at no more steps
  than the new visit, and every transition it left asleep also sleeps in
  the new visit (`NodeInfo::sleep ⊆ child_sleep`; merged explorations keep
  the intersection). This metadata sits **beside** the node, never in the
  state key: two prefixes reaching one state are the same state with
  different obligations.

`--source-dpor` (default configuration `stateful-sleep`) is recommended for
large models with only `final` invariants; it is the only search that
visits fewer distinct states than the unreduced one. Measured sizes:
[benchmarks](../development/benchmarks.md).

## Formal assumptions

States are equal when their canonical encodings are equal (exact bytes,
never a hash; [model semantics](../concepts/model-semantics.md#state-identity)).
Transitions are partial functions identified by `(kind, actor, task)`; the
system is acyclic within `maxSteps` (program counters, retry counters and
fault budgets only grow). For transitions `a`, `b` both enabled in `s` with
non-conflicting footprints, the explorers assume:

1. **Independence** — each stays enabled after the other;
2. **Diamond** — both orders reach the same state;
3. **Footprint stability** — `b` accesses the same resources after `a`;

and, for a transition that enables another, 4. **enabling pairs are
dependent** — enforced by the search itself, so a missing access cannot
hide it. The audit checks 1–3 for every independent pair and the footprint
half of 4 for every enabling pair in every reachable state of bounded
models.

## What is preserved

| property | sleep sets | classic / source-set DPOR |
| --- | --- | --- |
| reachable states, `always` invariants | preserved | **refused** (`dpor_inapplicable`; the report runs sleep sets) |
| terminal states, `final` invariants, counterexample existence and validity | preserved | preserved |
| stuck states and their kind (pool starvation, application wait cycle) | preserved | preserved |
| PostgreSQL deadlocks | ordinary `Deadlock` transitions | same |
| enabled unsupported behavior | preserved | preserved |
| truncation by `maxSteps` | preserved | preserved (all linearizations of a trace have one length; a cached state is reused only by a visit at no fewer steps) |
| `maxContextSwitches` | reduction off | **refused** |
| liveness, fairness | not claimed | not claimed |

Terminal states are compared **modulo the interleaving of external effects
on different channels** (`State::canonical_observable` sorts the log
stably by channel). Effects on one channel conflict and keep their order;
no invariant can tell cross-channel orders apart (`externals` counts per
channel) and no program reads the log. The visited-state identity inside
every search stays the exact encoding.

Not preserved, by any reduction: intermediate states (for DPOR), the
number of context switches, the number of distinct states visited
(`metrics.states` counts opened nodes), schedule counts, the *first*
counterexample found and the witness shape.

## How the reductions are checked

| check | where | what |
| --- | --- | --- |
| differential oracle | `tests/common/por_diff.rs` | against the unreduced search: terminal states (modulo cross-channel effect order), verdict, counterexample existence and replay, stuck states, unsupported behavior, step-bound truncation; a scenario whose unreduced search hits a limit is *unverified*, not passing |
| targeted workloads | `por.rs`, `dpor.rs`, `sdpor.rs` and the per-feature suites (`foreign_keys.rs`, `js_values.rs`, `drizzle_statements.rs`, `multi_row_locks.rs`, `nulls_not_distinct.rs`, `in_flight.rs`, `process_pool.rs`) | every algorithm against the unreduced search, at all three isolation levels |
| random scenarios | `por.rs` (`tests/common/scenarios_gen.rs`), regression seeds in `tests/data/por-regression-seeds.txt` | 2–3 actors on shared tables |
| adversarial generator | `por_adversarial.rs` (`tests/common/adversarial.rs`) | nine profiles that share exactly one resource and keep everything else private (rows, pool, tasks, in-process variables, sequence, fault budget, locks, SERIALIZABLE predicates, mixed), so the dependency under test is not masked; generated features are compared with *observed* features of the state space |
| metamorphic tests | `sdpor.rs` | renaming, reversed declaration order, an added pure step, an added independent actor preserve verdict, terminal-state count and stuck-state existence |
| independence audit | `por_audit.rs`, `explore::audit_independence` | in every reachable state: independent pairs commute and stay enabled, footprints do not drift, enabling pairs conflict |
| mutation adequacy | `por_mutation.rs` (feature `mutation-testing`) | deletes one access kind from every footprint and reports which kinds some workload notices; kinds no workload kills (`RowRead`, `LockCheck`, `Wait`, `Seq`, `Budget`) stay in the relation — deleting a conservative access is never the response to "no test notices" |
| regression fixtures | `por_regressions.rs`, `tests/data/regressions/` | one minimized scenario per defect found (below), checked both ways: every algorithm agrees now, and the recorded algorithm still disagrees with the defect switched back on |

Defects these checks found in the DPOR variants, each kept as a regression
fixture: a sleeping member of a choice class that did not make its siblings
obligations; the enabler treated as an ordered predecessor; only the last
race handled (this one was present in the first classic DPOR and lost a
terminal state on a nine-transition scenario with two actors in one process
and one crash); and a pool hand-over whose claim recorded no `Pool` access.
Measured campaign sizes are in the [evidence index](../research/evidence-index.md).

## Remaining proof obligations

1. **The summary argument**: that the exploration below a completed state
   is complete for reorderings of its own events independently of the
   prefix. Tested, not proved.
2. **Source sets with sleep sets and state-dependent enabling**: the
   source-set theorem assumes a fixed set of processes whose next event
   does not depend on others' events. Here enabling is conditional
   (`Crash`), choices are nondeterministic outcomes of one task, and
   footprints are state-dependent. Each refinement above was motivated by a
   failing test; there may be others the generators cannot produce.
3. **The audit's scope**: commutation is checked over the reachable states
   of bounded models; the relation is assumed to generalize to larger
   bounds.
4. **State equivalence**: the canonical encoding is assumed to contain
   everything that influences future behavior.

# Partial-order reduction

`crates/commitrift-model/src/footprint.rs` (independence),
`explore.rs` (sleep sets, independence audit).

## Strategy: sleep sets with state caching

Chosen over persistent sets and dynamic POR because:

* CommitRift's transitions are *heavy and conditional*: whether an `UPDATE`
  conflicts with a `COMMIT` depends on the rows, images and locks in the
  current state. Sleep sets need only a sound independence check between
  two transitions *that are both enabled in the current state*, computed from
  what each actually did there (its footprint). Persistent sets need a
  static over-approximation of all future transitions of every actor;
  DPOR needs happens-before tracking across backtracking with blocking and
  crash/retry transitions — both are more error-prone in this setting.
* With canonical-state caching, sleep sets visit every reachable state of the
  unreduced search (they prune *transitions*, not states). This preserves
  **every state predicate**: `always` invariants, `final` invariants in
  quiescent states, stuck states. That is exactly the property verdicts rely
  on.
* The implementation is small enough to be tested exhaustively against the
  unreduced explorer.

### Algorithm

From a state *s* explored with sleep set *S* (transitions whose
continuations are known to be covered elsewhere):

1. skip transitions in *S*, and transitions already explored from *s* in an
   earlier visit;
2. for the next transition *t*: the child's sleep set is every *u* in
   *S* ∪ (explored earlier from *s*) ∪ (explored before *t* in this visit)
   that is **independent of *t* in *s***;
3. record *t* as explored from *s*.

Each (state, transition) pair is explored at most once, so the reduced
search never explores more transitions than the unreduced one (asserted on
every differential test). The successors of sleeping transitions are still
computed once per expansion, because their footprints are needed for the
independence checks: the reduction saves the subtrees below them, not the
single step.

## Independence relation

Each transition records the abstract resources it read and wrote, as it
actually executed in the state (`Access`):

| Access | Recorded by | Conflicts with |
| --- | --- | --- |
| `Actor(a)` | every Step/Resume of actor *a* | every transition of *a* (shared locals, one connection per transaction) |
| `Pred(table, predicate)` | every scan | a `RowWrite` of that table whose old **or** new image matches the predicate |
| `RowRead(row)` | re-reading a newer version (EvalPlanQual) | `RowWrite` of the row |
| `RowWrite(row, old, new)` | writes; commits (publishing an image); aborts (removing one); the *intended* image of an insert or update even when it fails or waits | the same row, or any row sharing a non-null unique key with old/new |
| `LockCheck(row)` / `LockChange(row)` | lock tests / lock acquisition and release | lock changes of the row |
| `Snapshot` | taking a transaction snapshot; **any statement that ends up waiting** (it keeps its snapshot) | `CommitWrite` |
| `CommitWrite` | a commit that wrote | `Snapshot` |
| `Ser` | every statement, commit and abort of a SERIALIZABLE transaction | every other `Ser` |
| `Wait` | starting to wait, resuming, ending a transaction somebody waits for | every other `Wait` |
| `Ext(channel)`, `App(process, var)` | external effects (an ordered log), in-process variables | the same channel / variable |
| `Global` | crashes, retries, deadlock victims, unsupported operations | everything |

Two transitions are independent iff no pair of their accesses conflicts.
Unknown aliasing is never independence: an unsupported operation is
`Global`; a predicate that cannot be evaluated on an image conflicts.

Unsound shortcuts the tests found and that were fixed:

* an `INSERT … ON CONFLICT DO NOTHING` that hit a conflict recorded no
  footprint; it does not commute with an uncommitted `DELETE` of the
  conflicting row (it then *waits* instead) — the intended image is now
  always recorded (random seed 5);
* a READ COMMITTED statement that waits keeps its snapshot, so its position
  relative to other commits becomes observable — a waiting statement now
  reads the commit stream (seed 86, found by the independence audit).

## Preservation guarantee and its scope

Within the same bounds, the reduced search reaches the same set of
canonical states as the unreduced search; therefore the same invariant
violations are reachable and the verdict is the same. This holds when:

* `maxContextSwitches` is **not** set — a commuted ordering can need more
  context switches, so reduction is disabled automatically when it is
  (reported);
* the independence relation is valid in every reachable state (checked by
  the audit below, not proven).

Not claimed: preservation of schedule *counts*, of the *first*
counterexample found, of the witness shape, or of properties over paths
(temporal properties other than state invariants).

## Validation

| Test | What it checks |
| --- | --- |
| `por.rs: targeted_scenarios_preserve_every_reachable_state` | 14 workloads (reads, writes, locks, external effects, commits, crashes, retries, deadlocks, `SKIP LOCKED`, tasks, SERIALIZABLE): reduced = DFS = BFS reachable state sets; equal violating-state counts; never more transitions |
| `por.rs: randomized_scenarios_agree_between_full_and_reduced_exploration` | random scenarios (2–3 actors, all statement kinds, random isolation, transactions, rollbacks, failures, crashes, retries, unique keys, external channels with and without dedup); 150 seeds in the quality gate, 1 500 seeds measured; failing seeds go to `tests/data/por-regression-seeds.txt` |
| `por.rs: the_independence_relation_is_valid_on_random_scenarios` | the **audit**: in every reachable state, every pair of enabled transitions judged independent reaches the same canonical state in both orders and neither disables the other |
| `por.rs: unknown_resources_are_dependent`, `conflicting_writers_are_not_reduced_away`, `independent_actors_collapse_to_one_ordering` | targeted dependence cases |

Measured on 1 500 random scenarios: 0 disagreements, 0 audit violations;
transitions 327 430 (unreduced) → 265 902 (reduced), −19 %. Reachable states
are identical by construction of the guarantee. Per-workload reductions and
larger scenarios: [`PERFORMANCE.md`](PERFORMANCE.md).

# Formal assumptions of the reductions

What a reduction may skip is only as sound as the assumptions below. Each is
stated, then tied to the test or audit that checks it. "Checked" means a
finite check over bounded models, not a proof.

## The transition system

* **States** are `state::State`; two states are *the same state* when their
  canonical encodings (`State::canonical`) are equal. The visited-state table
  compares the exact encoding (interned fragments, exact keys; see
  `STATE-SPACE-PERFORMANCE.md`), never a hash.
* **Transitions** are identified by `(kind, actor, thread)`
  (`state::Transition`). A transition is a *partial* function of the state:
  `enabled(s)` is computed from the state, `apply(s, t)` is defined exactly
  when `t` is enabled. The identity does not say *which* step of a task it is
  (the next one), so one identity recurs along a run.
* Some transitions are alternatives of one choice (the server answers, the
  answer is lost and the operation reached the server, or it did not;
  the outcomes of a crashing process's in-flight operations). They are *not*
  concurrent processes.
* The system is acyclic within `maxSteps`: program counters, retry counters and
  fault budgets only grow. Polling and retry loops are cut by `maxSteps`
  and by the retry budget, not detected as cycles (`POR-CORRECTNESS.md`).

## Footprints and dependence

Every executed transition records the resources it read and wrote
(`footprint::Access`). Two footprints *conflict* by the table in
`footprint.rs`; `dependent(a, b)` is "some pair of accesses conflicts".
Unknown evaluation, crashes, retries, deadlock victims and unsupported
behavior are `Global`: dependent with everything. The relation is meant to be
a **superset** of semantic dependence. The explorers assume, for transitions
`a`, `b` both enabled in `s` whose footprints do not conflict:

1. **Independence (legal in both orders).** `b` is enabled after `a` and `a`
   after `b`.
2. **Diamond.** Both orders reach the same state, under the equivalence
   below.
3. **Footprint stability.** `b` accesses the same resources after `a` as
   before.

and, for a transition `t` that *enables* `x` (enabled after `t`, not before):

4. **Enabling pairs are dependent.** The explorers treat the pair `(t, x)` as
   dependent whatever the footprints say. This is the one assumption the
   search enforces itself, so that a missing access cannot hide it (the
   Phase 9 pool hand-over defect).

`audit::audit` checks 1–3 for every independent pair and the footprint half of
4 for every enabling pair over all reachable states of a bounded model
(`tests/por_audit.rs`: 84 077 independent pairs, 31 286 enabling pairs on 144
workloads, no violation, no footprint drift) and the same audit finds the
Phase 9 defect when the claim's pool access is deleted (11 workloads).

## The equivalence the reductions preserve

Reductions do **not** preserve the set of intermediate states, and they do not
preserve database equality alone. A reduced search and the unreduced search
must agree on:

| preserved | meaning |
| --- | --- |
| terminal states | the set of canonical states in which nothing is enabled, **modulo the interleaving of external effects of different channels** (below) |
| final-invariant verdicts | a function of terminal states |
| counterexample existence and validity | a counterexample is a schedule that replays on the model and violates the invariant |
| stuck states and their kind | quiescent states in which a task still runs: pool starvation, application wait cycle |
| PostgreSQL deadlocks | `Deadlock` is an ordinary transition |
| enabled unsupported behavior | |
| truncation by `maxSteps` | all linearizations of a trace have one length |

Terminal states differ in much more than database rows, and all of it is
part of the canonical state and therefore preserved: knowledge and possible
worlds (`Ambiguity`), pending server operations (`Orphan`), connection and
lease ownership (`Lease`, `PoolState`), task states and handlers, lock
ownership, retry and crash counters, in-process variables, locals and the
external-effect log.

**The one relaxation.** The external-effect log is ordered globally in the
canonical state. Effects on one channel conflict (`Access::Ext(channel)`), so
their order is preserved. Effects on different channels are independent: a
reduction explores one interleaving and does not distinguish the others.
No invariant can tell them apart (`externals` counts per channel), no program
reads the log and it never influences a later transition. `State::
canonical_observable` sorts the log stably by channel; it is the key under
which terminal states are compared between searches
(`ExploreResult::terminals`). The visited-state identity used inside every
search stays the exact canonical encoding. The adversarial campaign found
this on its first run (seed 6, two externals on two channels): the Phase 6–9
sleep-set search has always identified those states, the oracle was stricter
than the property.

**Not preserved**, hence refused or documented: intermediate states
(`always` invariants are refused), the number of context switches
(`maxContextSwitches` is refused), the number of distinct states visited
(`metrics.states` counts nodes the search opened), fairness and liveness.

## Reduction metadata is not state identity

The state table maps an exact canonical encoding to a node. What a search
additionally knows about a state — the sleep set it was explored under, the
summary of the events below it, the step count it was explored at — is
stored *beside* the node, never in its key, and a visit may reuse it only
under the stated conditions (`SOURCE-DPOR.md` §Reuse). Two prefixes that
reach the same state are the same state with different obligations.

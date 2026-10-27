# Enabling and disabling relations

The footprint relation was designed to say which transitions *commute*. Two
transitions can also fail to be independent because one changes whether the
other is enabled. Phase 9 found that case by accident (a waiting task's claim
of a handed-over connection recorded no access, so it was not ordered after
the release; one of four terminal states was missing from DPOR). This
document records how Phase 10 treats it.

## What enables what

| transition | becomes enabled by | other ways |
| --- | --- | --- |
| `Step` of a task waiting for a connection | the release that hands the connection over (a COMMIT, ROLLBACK or the end of a task) | with pool capacity > 1, any of several releases |
| `Resume` of a blocked statement | the end (commit/abort) of the transaction it waits for | one blocker |
| `ServerComplete` | the `LostCommitted` / crash outcome that put the operation on the server | one cause |
| `Retry` | the failure or crash it reacts to | |
| `Crash` of a process | the first visible operation of **any** running task of the process | **disjunctive**: whichever task starts first |
| `Deadlock` | the wait that closes a cycle | |
| child task `Step` | the `spawn` of its parent | |

`Crash` is the case that matters: it is enabled only while some task of the
process has started and not finished, so after `S0` in a process with tasks
`T0`, `T1` it may be disabled, and after `S1` enabled. Several independent
events can each be the one that enables it.

## Rules the explorers follow

1. **An enabling pair is dependent.** For a transition `u` about to be
   executed in the state after trace `E`, the *enabler* is the last event
   after which `u` became enabled and stayed enabled (the last `k` with
   `u ∉ enabled(state before e_k)`); it is computed from the enabled sets the
   search holds anyway (`enabling_event`). Its pair with `u` is added to the
   dependence relation, so it is both a happens-before edge and a race
   candidate, whatever the footprints say.
2. **It is never an ordered pair.** An earlier design (and the historical
   defect `DEFECT_ENABLER_ORDERS`) took the enabler as a necessary cause and
   removed its race with `u`. That is wrong whenever enabling is disjunctive:
   it hid the race whose reversal runs the *other* enabling task first, and
   the search lost every crash that follows the second task's first step
   (found on shared processes without a pool; fixture
   `enabler-orders-the-enabled-adversarial-*.json`).
3. **Disabling** is handled where it happens: a transition that `t` disables
   (enabled before, not after) is added as an alternative at the state before
   `t`, because no later race can ask for it. A crash disables every task of its
   process; the outcomes of a pool release race for the connection through
   `Access::Pool`.
4. **Alternatives of one choice** are explored together. A *sleeping* member
   counts: sleeping says its own outcome is covered elsewhere, not that its
   siblings are (`DEFECT_SLEEPING_CHOICE`, random seed 87).
5. **All races, not the last.** With conditional enabling the earlier race is
   the one that matters (`DEFECT_LAST_RACE_ONLY`; the classic Phase 9 search
   lost a terminal state on a nine-transition scenario with two actors in one
   process, one crash and no pool, and is fixed accordingly).

## How this is tested

* `audit::audit` checks, for every enabling pair of every reachable state,
  that the footprints already conflict (they do on the whole corpus), and
  counts the pairs where two independent transitions both enable the same
  third one (74 such pairs: the disjunctive case actually occurs).
* `tests/por_mutation.rs` deletes the claim's pool access (the Phase 9
  defect) and one kind of access at a time from every footprint; the
  Phase 9 search configuration loses terminal states, the new searches do not.
* `tests/data/regressions/` holds one minimized scenario per defect above,
  each checked both ways (handled by every algorithm now; still exposing the
  defect with the defect switched back on).

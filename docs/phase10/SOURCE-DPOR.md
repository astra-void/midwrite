# Source-set DPOR with sleep sets and a summarizing state cache

`crates/commitrift-model/src/explore/sdpor.rs` (`explore_sdpor`), selected with
`--source-dpor [stateful-sleep|stateful|stateless-sleep]` or
`"options": {"sourceDpor": "..."}`. Sleep-set reduction (Phase 6) stays the
default.

## Algorithm choice

| candidate | verdict |
| --- | --- |
| Optimal DPOR (wakeup trees) | not chosen. Its optimality is about executions per Mazurkiewicz trace; the profile (`BASELINE.md`) shows the cost here is traces *and* states, and wakeup trees would add a correctness surface around state-dependent enabling (conditional `Crash`, pool hand-over, choice alternatives) that nothing here needs |
| **Source-set DPOR (Abdulla et al., POPL 2014)** with sleep sets | **chosen** for backtracking: when a race is found, one *initial* of the reordered sequence is added only if the backtrack set holds none already |
| stateful search with per-state summaries (Yang et al., SPIN 2008) | **chosen** for state caching: a state is not explored twice, and the races of the events below it are replayed against the new trace from a stored summary |
| persistent sets from static future footprints | not chosen: the interesting independence is row-level and value-dependent, which a static over-approximation of a task's future accesses does not see |
| classic DPOR | kept (`--dpor`), now with the every-race rule; reference for the comparisons |

All three mechanisms (`cache`, `sleep`, source sets) can be switched on
separately (`SdporConfig`); the differential tests run every configuration.

## The search

A depth-first search over a trace `E = e_1 … e_n`, one frame per state with
the enabled transitions applied once (footprints and successor states), a
*backtrack set*, the transitions *done*, and a *sleep set*.

1. **Happens-before.** Event `e_j` happens after `e_i` if their footprints
   conflict, if they are events of the same task (program order), or if `e_i`
   enabled `e_j`; closed transitively (`TraceEvent::before`, bit sets).
2. **Races.** At a new frame, for each enabled transition `u`: every event
   `e_i` that is dependent with `u` (or enabled it) and not already ordered
   before `u` through another predecessor, and not an earlier event of `u`'s
   task, is a race `(e_i, u)`. *Every* such event is handled (the classic
   search handled the last only, which loses runs when enabling is
   conditional: `DEFECT_LAST_RACE_ONLY`).
3. **Source sets.** `notdep(e_i).u` is the events after `e_i` that do not
   happen after it, then `u`. Its *initials* — events with no predecessor in
   the sequence, necessarily enabled before `e_i` — are the transitions that
   can start the reordering. If the backtrack set of the state before `e_i`
   already contains an initial that is awake, nothing is added; if an awake
   initial exists, one is added; if every initial sleeps the run is covered by
   the exploration that put it to sleep; if none is enabled there (the
   transition was not enabled before `e_i`), every awake enabled transition is
   an alternative. No transition is added merely because it is the racing one.
4. **Choice alternatives.** If the backtrack set (or the sleep set) holds a
   member of a choice class, every awake member is added
   (`DEFECT_SLEEPING_CHOICE`).
5. **Disabled alternatives.** Executing `t` adds the transitions it disables
   to the backtrack set of its state.
6. **Blocked steps** are dependent with everything until their `Resume`.
7. **Sleep sets.** The child of `t` sleeps on every transition that sleeps in
   the parent or was done before `t`, and is independent of `t`.
   A frame whose backtrack set holds only sleeping transitions is popped
   ("sleep-set blocked").
8. Frames pop when no awake backtrack transition remains.

## The state cache (`cache`)

Plain stateful DFS is unsound for DPOR: the races between events above a
state and events below it were detected on the first visit, against a
different prefix. Each completed state therefore stores a **summary**: the
interned set of `(transition, footprint)` descriptors executed anywhere below
it (a summary is the union of the summaries of its children and its own
edges). When the search reaches a completed state again it does not recurse;
it runs the race detection of every summarized event against the current
trace as if that event were enabled there, adding backtrack points to the
frames on the stack (candidates: the event's transition and the tasks of the
events that happen before it, the set `E` of Flanagan–Godefroid, because the
initials of the unexplored continuation are not known), and unions the
summary into the frame above. The detection ignores the happens-before chains
that run through events below the cached state, so it finds at least the
races the full continuation would (it may find more: sound, wasteful).

A summary is sound to use for the races of the *prefix* because the
exploration below a state is complete for every reordering of events *below*
it (their races were detected inside), and every race between a prefix event
and a lower event is, by construction, reversed in the prefix. This is an
argument, not a proof; `POR-CORRECTNESS.md` lists what supports it.

### Reuse conditions (state identity vs reduction metadata)

A visit reuses a completed exploration of the same state only when

* the state is completed (a state still on the stack is explored again: a
  cycle within the step bound is followed to the bound, as the classic search
  did);
* it was explored at `State::steps` no greater than the new visit's (a
  shallower visit has more room below `maxSteps` and explores again);
* every transition the stored exploration left **asleep** also sleeps in the
  new visit (`NodeInfo::sleep ⊆ child_sleep`); when two explorations are
  merged the stored sleep set becomes their intersection.

None of this is part of the state key.

## Cost model

* Per frame: every enabled transition applied once (as in the sleep-set
  search); race detection is `O(|enabled| · n)` footprint comparisons plus
  `O(n²)` per race for the initials.
* Per cache hit: `O(|summary| · n)` comparisons; summaries are interned, so
  states that have the same events below them share one.
* Memory: the same compact key table as Phase 9, plus the summary pool and
  the descriptor table (reported as `reductionMetrics.summaryBytes`) and the
  stack frames.

## What it preserves

See `FORMAL-ASSUMPTIONS.md` (terminal states modulo cross-channel effect
order, final-invariant verdicts, counterexamples, stuck states, deadlocks,
enabled unsupported behavior, step-bound truncation). Refused, with the
reason in the report: `always` invariants and `maxContextSwitches`.
`metrics.states` counts the distinct states the search opened.

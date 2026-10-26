# Dynamic partial-order reduction

`crates/commitrift-model/src/explore.rs` (`explore_dpor`), selected with
`--dpor` or `"options": {"dpor": true}`. Sleep-set reduction (Phase 6) stays the
default.

## Choice of algorithm

| candidate | verdict |
| --- | --- |
| Source-DPOR / Optimal DPOR (Abdulla et al.) | needs the *initials* of a trace and wakeup trees; a larger correctness surface than the transition model of this checker (nondeterministic choices, state-dependent footprints) justifies |
| stateful DPOR with cycle handling | the visited-state cache is exactly what makes DPOR unsound without care (a revisited state hides backtrack points); not attempted |
| **classic DPOR (Flanagan–Godefroid 2005), stateless, no sleep sets** | **chosen**: small, the soundness argument is the textbook one plus four explicit refinements below; memory is the stack |

Stateless means no table of visited states: memory is the frames on the stack
(`dpor_max_frames` in the metrics) instead of one key per reachable state. The
price is that two interleavings that merely *converge* to the same state are
both explored. DPOR reduces equivalent *traces*, the state cache merges
equivalent *states*; neither subsumes the other (`PERFORMANCE.md` measures
both).

## The algorithm

A transition is `(kind, actor, thread)` (`state::Transition`); the explored
object is a depth-first path `e_1 … e_n` of executed transitions with their
footprints (`footprint.rs`, the same dependence relation the sleep sets and the
independence audit use).

1. **Happens-before.** When `e_k` is executed, `before[k] = ⋃ {i} ∪ before[i]`
   over earlier events `i` whose footprint is dependent with `e_k`'s.
2. **Races.** After a state is reached, every enabled transition `u` is
   compared with the trace: `D = {i : e_i dependent with u}`; the events that
   are *ordered* before `u` through another dependent event are
   `⋃_{k∈D} before[k]` plus the earlier steps of the same task (program order);
   the last event of `D` not ordered is a reversible race `(e_i, u)`.
3. **Backtracking.** If `u` is enabled in the state before `e_i`, add `u` to
   that state's backtrack set; otherwise add every transition enabled there
   (always sound, sometimes wasteful).
4. **Exploration.** A state explores only its backtrack set, seeded with its
   first enabled transition; each transition once (`done`).

## Refinements this transition system needs

Found by the differential tests (`tests/dpor.rs`); each was a real missed
terminal state before it was fixed:

1. **Program order is per task.** Two transitions of one task are sequenced by
   its program and are not a race; transitions of *different* tasks of one
   actor are (tasks `Promise.all`). Without this the search missed the
   interleavings in which a later task step precedes an earlier one's
   conflicting step (lost-update workload: 2 of 3 terminal states).
2. **Disabled transitions become alternatives.** If the transition chosen at a
   state disables another enabled one (finishing an actor disables a crash),
   the disabled one can never be requested by a later race: it is added to the
   state's backtrack set. (Crash-retry workload: a crash between COMMIT and the
   next operation was missing.)
3. **A blocked step is dependent with everything.** A statement that starts
   waiting for a lock has not done its work; the footprint of the step does not
   show what the completed statement conflicts with in another order. Only
   its `Resume` does. (Random seed 20: a read ordered after a delete that
   resumed later was missing.)
4. **Alternatives of one choice are explored together.** The server's
   alternative behaviors for one operation (the answer, a lost
   acknowledgment that reaches the server or not) and the outcomes of a
   crashing process's in-flight operations are alternatives of *one* process,
   not concurrent processes; DPOR's "one next transition per process" does
   not hold for them, so choosing one explores the siblings (random seed 30).

Footprints of crashes, retries and server completions are conservative
(`Access::Global` for crashes and retries; the rows a completion commits plus
the submitting actor for `ServerComplete`), so reduction is weak when faults
are on: it is not claimed to help there.

## What is preserved, and what is not

Preserved (all functions of the final state of a trace): the set of terminal
states — hence every `final` invariant violation, counterexample existence —,
stuck states and their classification, enabled unsupported behavior, deadlock
reachability (`Deadlock` transitions are ordinary transitions), truncation by
`maxSteps` (all linearizations of a trace have one length).

**Not preserved:** the intermediate states of the interleavings that are not
explored. Therefore:

* a model with an `always` invariant is refused (`dpor_inapplicable`); the
  report says so and runs the sleep-set search (`bounds.partialOrderReductionDisabledBecause`);
* `maxContextSwitches` disables it (a commuted ordering can need more
  switches);
* `metrics.states` counts visited nodes (not distinct states) and `maxStates`
  bounds them;
* **liveness and cycles:** the transition system is acyclic within `maxSteps`
  (program counters and fault budgets only grow); a retry loop is bounded by
  `maxRetries`, polling by `maxSteps`. Nothing here is a liveness check; DPOR
  preserves only safety (final-state) properties and stuck-state reachability
  of bounded executions. Bounded cycles are *not* discarded: a path that
  re-enters a state is simply followed again until the step bound.

## Validation

`tests/dpor.rs` compares, on every scenario that is small enough to exhaust,
the **set of terminal states** of the unreduced search, the sleep-set search
and DPOR, plus verdict, counterexample existence, stuck states, unsupported
behavior and step-bound truncation, and replays DPOR counterexamples on the
model:

* 14 targeted workloads (lost update at three isolation levels, `FOR UPDATE`,
  write skew, redeem with and without a constraint, deadlock, skip locked,
  crash/retry, transaction retry, concurrent tasks);
* 150 deterministic random scenarios (+ the regression seeds file), with
  in-flight faults on a third of them; and a 700-seed run in release mode
  (`reports/phase9/dpor-700-seeds.txt`).

The independence audit (`audit_independence`) covers the footprint relation
for the new transitions (`phase9_inflight`, `phase9_process`). No claim beyond
these: DPOR is evidence-checked, not proved, for this transition system.

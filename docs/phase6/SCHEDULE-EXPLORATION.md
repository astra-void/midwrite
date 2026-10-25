# Schedule exploration

`crates/commitrift-model/src/explore.rs`.

## Algorithms

| Strategy | Use |
| --- | --- |
| Depth-first with sleep sets (default) | partial-order reduction ([`PARTIAL-ORDER-REDUCTION.md`](PARTIAL-ORDER-REDUCTION.md)) |
| Depth-first without reduction (`--no-por`) | the reference for differential tests |
| Breadth-first without reduction (`--no-por --strategy bfs`) | shortest counterexamples first |

All three use the same transition system ([`SEMANTICS.md`](SEMANTICS.md)),
deterministic transition order, and the same canonical-state deduplication.
The depth-first search is iterative (an explicit stack): no recursion limit.

Per state, the explorer:

1. evaluates `always` invariants the first time the state is reached;
2. if the state is quiescent: evaluates `final` invariants (or records a
   stuck state);
3. stops at `maxSteps` (recorded as truncation);
4. applies every enabled transition once to obtain successors and their
   footprints; an unsupported transition is recorded and has no successor.

A state already expanded at a smaller or equal depth is not expanded again
for transitions it already explored; a shallower revisit after a deeper one
re-expands it (so the step bound cannot hide a state).

## Bounds

| Bound | Kind | Default | When reached |
| --- | --- | --- | --- |
| `maxActors` | scenario | 4 | the scenario is rejected |
| `maxCrashes` | semantic | 0 | no further crash transition |
| `maxRetries` | semantic | 0 | no further operation or transaction retry |
| `maxContextSwitches` | semantic | none | longer schedules are not explored (reported); disables reduction |
| `maxSteps` | resource | 64 | path cut; `INCONCLUSIVE` |
| `maxStates` | resource | 200 000 | search stops; `INCONCLUSIVE` |
| `maxTransitions` | resource | none | search stops; `INCONCLUSIVE` |
| `maxWallMs` | resource | none | search stops; `INCONCLUSIVE` |
| `maxMemoryBytes` | resource | none | approximate size of the visited table (keys + 96 bytes each); `INCONCLUSIVE` |

The report lists the declared bounds, whether reduction was on (and why
not), and every truncation flag. `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` is only
possible when no resource bound was reached, no unsupported operation was
reachable and no state was stuck (tested:
`hitting_a_state_limit_is_never_reported_as_exhaustive`,
`a_depth_cut_is_inconclusive_not_exhaustive`).

## Deduplication

States are stored by their full canonical encoding (no hash-only
comparison, so no collision can merge different states). What is in the
encoding and why: [`SEMANTICS.md`](SEMANTICS.md#state-equality).

## Metrics

`states` (distinct canonical states), `transitions` (executed),
`terminalStates` (quiescent states), `duplicateStateHits`,
`sleepPrunedTransitions`, `contextPrunedTransitions`, `maxDepth`,
`schedulesInExploredGraph` (number of maximal paths in the explored state
graph; for an unreduced search this is the number of distinct schedules up
to state merging), `visitedBytes`; wall time and time to the first violation
are reported separately (`timing`, omitted with `--no-timing` for
reproducible output).

## Differential schedule enumeration

`witness::enumerate_schedules` enumerates every complete schedule (no
merging, no reduction) for the PostgreSQL matrix; it is exponential and used
only on small workloads.

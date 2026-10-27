# Correctness evidence for the reductions

Nothing here is a proof. The reductions are **evidence-checked** against the
unreduced search, which does not use footprints at all, on bounded models;
the dependence relation they rely on is **audited** state by state; and the
test corpora are **measured** for what they can detect. This file lists the
evidence and what remains a proof obligation.

## 1. What is compared

Against the unreduced depth-first search, for every algorithm (sleep sets,
classic DPOR, source-DPOR stateful / stateless+sleep / stateful+sleep /
stateless): terminal states modulo the interleaving of effects on different
channels; verdict; counterexample existence; that the counterexample replays
on the model and violates its invariant; stuck-state existence; enabled
unsupported behavior; truncation by `maxSteps`. A scenario whose unreduced
search hits a resource limit is **unverified** and counted separately.

## 2. Corpora and results (macOS, release builds)

| suite | scenarios | result |
| --- | ---: | --- |
| targeted Phase 6–9 workloads (`tests/sdpor.rs`) | 14 | all agree |
| Phase 9 model scenarios + pool/crash/in-flight workloads | 10 | all agree |
| old random generator, seeds 1–150 + regression seeds | 149 compared, 1 unverified | all agree |
| adversarial generator, 360 seeds (`tests/por_adversarial.rs`, default) | 360 | all agree |
| adversarial generator, 3 000 seeds (evidence run) | 3 000 compared, 0 unverified | all agree |
| metamorphic: rename, reversed declaration order, a pure step, an independent actor | 60 + 60 + 60 + 28 | all preserve verdict, terminal-state count and stuck-state existence |
| regression fixtures, both directions | 7 | handled by every algorithm; each still exposes its defect when the defect is switched on |
| audit (`tests/por_audit.rs`) | 144 workloads, 84 077 independent pairs, 31 286 enabling pairs | 0 violations, 0 footprint drift |

## 3. Defects found by this evidence

| # | where | found by | what was lost | fixed by |
| --- | --- | --- | --- | --- |
| 1 | source-DPOR (development) | old random seed 87 | a terminal state: a sibling outcome of a lost acknowledgment below a state where the answered outcome slept | sleeping choice members count (`DEFECT_SLEEPING_CHOICE`) |
| 2 | source-DPOR (development) | hand-written shared-process workloads, then the mutation test | terminal states after a crash that follows the *second* task's first step | the enabler is dependent, never an ordered predecessor (`DEFECT_ENABLER_ORDERS`) |
| 3 | **classic DPOR of Phase 9** and source-DPOR at the time | adversarial seeds 165, 75, 245 | a terminal state on a 9-transition scenario (two actors, one process, one crash, no pool) | every race, not the last only (`DEFECT_LAST_RACE_ONLY`); the classic search is fixed the same way |
| — | the oracle | adversarial seed 6 | nothing: the unreduced search distinguished the global order of independent effects | comparison modulo cross-channel effect order |

Defect 3 is a soundness bug in the Phase 9 DPOR, which `docs/phase9/
DYNAMIC-POR.md` described as preserving every terminal state. The Phase 9
random generator could not reach it (no shared processes); the adversarial
generator found it within its first 360 seeds.

## 4. Property preservation

| property class | sleep sets | classic / source-DPOR |
| --- | --- | --- |
| `final` invariants, counterexample existence | preserved (tested) | preserved (tested) |
| `always` invariants (intermediate states) | preserved: sleep sets with the state cache visit every reachable state (Phase 6) | **refused** (`dpor_inapplicable`); the report says why and runs sleep sets |
| PostgreSQL deadlocks | ordinary `Deadlock` transitions | same (tested: 148 generated models reach one) |
| stuck states (pool starvation, application wait cycles) | preserved (tested) | preserved (tested) |
| `maxContextSwitches` | reduction off | **refused**: a commuted ordering can need more switches |
| truncation by `maxSteps` | preserved | preserved; a cached state explored at more steps is reused only by a visit at no fewer steps |
| liveness, fairness | not claimed | not claimed |

**Cycles.** The transition system is acyclic within `maxSteps` (program
counters and budgets only grow); retry loops are bounded by `maxRetries`,
polling and connection reacquisition by `maxSteps`. A state on the current
stack is never taken from the cache (only completed states are), so a cycle in
the canonical-state graph is followed to the step bound, as the classic search
did. Nothing here checks a liveness property; "no counterexample" means no
reachable violating terminal or stuck state within the bounds.

## 5. Completeness reporting

Reports keep the existing verdicts: `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` only when
no resource limit was hit, no unsupported behavior was enabled and no stuck
state was found; `COUNTEREXAMPLE_FOUND`; `INCONCLUSIVE` for any truncation;
`UNSUPPORTED`. A reduced search reports `complete` only under those
conditions, and only for the properties of §4: a model with an `always`
invariant or a context-switch bound never runs a DPOR.

## 6. Remaining proof obligations

1. **The summary argument** (`SOURCE-DPOR.md`, state cache): that the
   exploration below a completed state is complete for reorderings of its own
   events independently of the prefix, so that replaying the races of its
   summary against a new prefix suffices. Tested, not proved.
2. **Source sets with sleep sets and state-dependent enabling**: the
   Abdulla et al. theorem assumes a fixed set of processes whose next event
   does not depend on others' events. Here enabling is conditional
   (`Crash`), choices are non-deterministic outcomes of one task, and
   footprints are state-dependent. The refinements in `ENABLING-RELATIONS.md`
   are each motivated by a failing test, which is how defects 1–3 were found;
   there may be others the generators cannot produce.
3. **The audit's scope**: it checks commutation and enabling pairs over the
   reachable states of bounded models; the footprint relation is assumed to
   generalize to larger bounds.
4. **Equivalence of states**: the canonical encoding is assumed to contain
   everything that influences future behavior (Phase 6 assumption, unchanged).

# Epistemic semantics

## The question

"For every physical history consistent with what the client observed, retrying
must not produce a second effective charge." The explorer answers it by
quantifying over **every modeled alternative**: each lost acknowledgment is a
branch (`LostCommitted`/`LostNotCommitted`, `CrashInFlight` masks, and the
point in time of `ServerComplete`), and invariants are checked in every
terminal state of every branch. This is a bounded quantification over the
declared fault model, not over real-world executions.

## Representation

Possible worlds are **explicit trajectories that share their prefix** in the
explored state graph, not copies of whole states per world:

* the two physical outcomes of one operation are two successors of one state;
* what the application knows is `ActorState::ambiguous`:
  `Ambiguity { seq, op, world, possible, attempt, via }` — the physical world
  of *this* trajectory and the set of worlds it cannot exclude;
* the application's *observation history* is its locals, outcome, attempt and
  later reads — two trajectories are observationally equal for an actor while
  those agree.

A symbolic outcome variable would need the engine to evaluate a statement
against both worlds at once. Explicit branching keeps every transition
unchanged, and the cost is the branching itself, which the reduction and the
compact state key address.

## Canonicalization

Two states with the same rows but different unresolved outcomes are *not*
merged. The canonical encoding carries, when anything is unresolved: the
fault budget used, every orphan (actor, id, transaction, statement, attempt),
every ambiguity (id, operation, physical world, possible worlds, attempt, how
lost). They are future-relevant for the report (the knowledge) and, through
the orphans, for the behavior (the server's completion). Tests
(`phase9_inflight`):

* `states_that_differ_only_in_unresolved_outcomes_are_not_merged`: the same
  state with the physical world flipped, the possible worlds narrowed, or the
  record dropped has a different encoding;
* `a_held_commit_is_part_of_the_state_until_the_server_completes_it`;
* `the_compact_table_identifies_exactly_the_states_of_the_flat_encoding`
  (`phase9_storage`): the interned key identifies exactly the same states as
  the flat encoding, on targeted and random scenarios, for BFS, DFS and
  sleep-set search.

Knowledge is kept per attempt and never cleared at a retry: a retried client
that finishes in the committed world and one that finishes in the aborted world
are different histories even if their rows agree.

## Knowledge refinement

What *can* eliminate a world is an observation that differs between worlds
(a read returns a row in one and not in the other, an insert hits a unique
constraint in one and not in the other). The model does not eliminate a world
by transition: the branches simply behave differently, and the twin analysis
(`twin.rs`) reports, for a counterexample, per lost acknowledgment:

* the twin world's schedule (the server's other behavior; an operation the
  twin sends to the server completes right after the fault), replayed strictly
  on the same model — `replayed`, or `diverges` with the reason;
* the invariants in both worlds (`actualVerdicts`/`twinVerdicts`);
* how many of the actor's observations the worlds share, and the first one
  that differs (`identicalObservations`, `firstDifference`);
* a sentence: whether the application could ever tell the worlds apart, and
  whether the invariant outcome differs between them ("the same decision is
  safe in one and unsafe in the other").

An observation eliminates a world **only** under the supported semantics: a
read returning `null` does not eliminate the world in which the server will
still complete the held COMMIT (`reading_durable_state_does_not_make_the_retry_safe`);
that is exactly why the reconciliation read is not sufficient evidence.

## Counterexample explanation (example)

```
Application knowledge at each lost acknowledgment:
  transition 4 — COMMIT of Client: PostgreSQL committed (reported schedule);
                 application knowledge: ambiguous: committed or not committed
    other world (not committed): replayed
    invariant `charged-once`: VIOLATED here, holds in the other world
    the application's observations are identical in both worlds for the rest
    of the schedule: it can never learn which one it is in; the invariant
    outcome differs between the worlds: the same decision is safe in one and
    unsafe in the other
```

The unsafe step is the retry: with identical knowledge in both worlds, any
retry decision is the same decision in both, so a retry that is correct in
world A and wrong in world B is wrong *for the application*.

## Not claimed

No claim about observations the model does not express (timing, error
messages, driver-level detail); the knowledge of a *different* actor about the
lost operation is not modeled (other clients see only the database).

# Phase 13 limitations

What the model and the extractor still do not represent, what they
approximate, and what was not measured. Phase 12's limitations that Phase 13
did not change still apply (`docs/phase12/LIMITATIONS.md`), except where
this file says they are lifted.

## Lifted in Phase 13

* Foreign-key locks (Phase 12's measured divergence): modeled for `NO
  ACTION`/`RESTRICT`, immediate constraints (`POSTGRESQL-FK-LOCKS.md`).
* Return values of the main task, and a crash between the last statement
  and the response (`RETURN-VALUES-AND-RESPONSES.md`).
* Read-replica database handles (`withReplicas` from drizzle-orm, or a
  declared project factory) and replica staleness (`REPLICA-CONSISTENCY.md`).
* `INSERT … ON CONFLICT (target) DO UPDATE`, `.for("share" | "key share" |
  "no key update")` (`ADVANCED-DRIZZLE.md`).
* Exported `const` function aliases; `bcryptjs`/zod calls are named by a
  contract instead of an anonymous package call (`SOURCE-RESOLUTION.md`,
  `PACKAGE-CONTRACTS.md`).

## Refused (the scenario does not load; never approximated)

* `ON DELETE`/`ON UPDATE` `CASCADE`, `SET NULL`, `SET DEFAULT` when a
  statement would run them; `DEFERRABLE INITIALLY DEFERRED` foreign keys
  when the referencing table is written.
* An invariant over the return value of an actor whose return expression is
  not modeled.
* Replica reads without the `replicas` feature; writes or row locks on a
  replica.

## Not modeled

* `SET CONSTRAINTS`, `MATCH FULL`, partial-index upsert arbiters
  (`targetWhere`), `SKIP LOCKED`/`NOWAIT` with a shared strength.
* Several replicas with different lags, synchronous replication, replica
  conflicts with recovery (hot-standby query cancellation).
* Error responses as a separate step (only normal returns respond); HTTP
  framework internals (the coordinator plays the client).
* Row values of `UPDATE`/`DELETE … RETURNING` (counts and presence are
  modeled), multi-row `values([...])`, string concatenation, arrays,
  `Map`/`Set`, timestamp columns, higher-order wrappers whose callback
  argument mapping is not known (`withRetry(() => …)`), inherited methods.
* `undefined` and `null` are one value.
* Return-value properties are never inferred: they must be declared (the
  frozen evaluation set declares none, so its observable positives are false
  negatives).
* The response step is inserted only for actors declared (or, in discovery,
  found) to be retried by their caller after a crash.

## Approximations, stated

* A waiting statement waits for the first conflicting lock holder, then
  re-checks: PostgreSQL's multixact member order. Deadlock detection sees
  that one wait-for edge (as PostgreSQL's detector does).
* Replica reads are treated as dependent on every primary commit by the
  partial-order reductions (sound, not minimal).
* `generated-values-distinct` (Phase 12) still holds: no collision mode.

## Not measured / not independent

* **No independent human review performed.** The frozen evaluation set and
  the PostgreSQL ground truth were produced by an AI agent without access to
  the implementation; that is independence from the code, not from the
  project.
* No PostgreSQL streaming replication was run: replica staleness is a model
  result only.
* Original-code verification covers first-party fixtures only; no corpus
  application was executed, and no corpus finding is claimed as a defect.
* Windows was not run.

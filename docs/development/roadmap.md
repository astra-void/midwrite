# Roadmap: open problems

Known gaps, collected from the limitations and evaluations. This is not a
schedule or a commitment. The full list of what is unsupported is
[limitations](../reference/limitations.md).

## Source coverage (blocks recall on real code)

From the frozen evaluation sets and the corpus
([evaluation datasets](../research/evaluation-datasets.md),
[corpus blockers](corpus-evaluation.md)):

* zod refinements (`refine`, `superRefine`), transforms, preprocess,
  coerce and unions — the current blocker of the private school-ledger
  project ([zod](../reference/zod.md#limitations));
* higher-order wrappers with unknown callback mapping, mutable escaped
  closures, effectful module factory initialization, inherited methods in
  discovery. Static analysis follows immutable captures and stable bindings
  initialized by inert factories ([value analysis](../architecture/value-analysis.md#interprocedural-summaries));
* `values(obj)` of a parameter object, array mutation, row values of
  `UPDATE`/`DELETE … RETURNING`, `orderBy` other than the primary key,
  multi-row mode with joins;
* request objects beyond one `request.json()`; Server Actions with
  `FormData`, tRPC;
* errors from lost connections and application error classes;
* return-value properties: today they must be declared; none is inferred;
* opaque fresh values (bcrypt hashes) so that package calls with verified
  contracts can be represented;
* more than two actors in synthesized scenarios; combinations of input
  relationships.

## Model and PostgreSQL semantics

* `ON DELETE CASCADE`/`SET NULL`/`SET DEFAULT`, deferred constraints,
  `SET CONSTRAINTS` (refused today)
  ([PostgreSQL model](../architecture/postgres-model.md#refused-or-not-modeled));
* partial and expression unique indexes; collation-aware equality;
* several replicas with different lags, synchronous replication, and a
  measured counterpart for the model-only replica results
  ([read replica](../architecture/postgres-model.md#read-replica));
* server-side timeouts as fates of a held COMMIT;
* pool generators for random differential testing and start-up arrival
  orders;
* a correctness argument for the reductions: the summary argument, source
  sets with sleep sets under conditional enabling, generalization of the
  audited relation, completeness of the canonical state
  ([remaining proof obligations](../architecture/partial-order-reduction.md#remaining-proof-obligations));
* more PostgreSQL versions (16, 14 not run) and plan shapes; SERIALIZABLE
  beyond the one-way abstraction; physical lock order of multi-row locks.

## Runtime verification

* Drivers beyond node-postgres 8.23.1 and postgres-js 3.4.5; postgres-js
  with pools, REPEATABLE READ/SERIALIZABLE and shared processes.
* Pool capacity > 1, application wait cycles and real deadlocks on
  original code.
* Connection loss beyond a closed socket for a COMMIT or autocommit write
  ([fault injection](../architecture/fault-injection.md#not-covered)).
* Linux runs of the later original-code evidence.

## Evidence and release

* **Independent human review** of findings, labels and semantics (none has
  been performed).
* An authorized reproduction in an external application with human
  review.
* Release blockers: a verified remote; a green GitHub CI run including
  required Linux replay; a tagged release run; a security review
  ([open items](../reference/security.md)); a human-reviewed precision
  baseline; crates.io and npm packaging decisions. A local Git history
  exists; no GitHub CI run, push or published package is recorded.
* Windows and x86_64 runs.

## Documentation

* Keep the topic documents current: update
  [limitations](../reference/limitations.md),
  [evidence index](../research/evidence-index.md) and the
  [CLI reference](../reference/cli.md) with each change, and run
  `python3 scripts/check-doc-links.py`.

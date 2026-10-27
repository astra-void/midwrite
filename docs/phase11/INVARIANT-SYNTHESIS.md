# Property candidates and their provenance

Application requirements cannot in general be read from code. Discovery
therefore attaches to every candidate one or more **property candidates**,
each with an origin that no later step changes, and reports a model
counterexample in terms of that origin.

## Origins

| origin | meaning | produced by |
| --- | --- | --- |
| `SCHEMA_ENFORCED` | a supported database constraint in the analyzed schema covers the property | a non-partial unique set (primary key, `.unique()`, `unique().on`, `uniqueIndex().on` without `.where`) covering the checked key |
| `SOURCE_ASSERTED` | an explicit assertion in source with a stated translation | **not produced in Phase 11** (see below) |
| `USER_DECLARED` | written in `commitrift-contracts.json` | templates instantiated with synthesized values |
| `FRAMEWORK_CONTRACT` | follows from a declared integration contract | at-least-once delivery ⇒ "a message causes at most one effect" |
| `HEURISTIC_HYPOTHESIS` | plausible, not established | every other property |

A partial unique index is reported as a protection that is *not* a global
constraint; it never yields `SCHEMA_ENFORCED`. A missing constraint is not
evidence that uniqueness was intended: the rationale of a check-then-act
hypothesis says the code's check *suggests* it and that "no supported
UNIQUE constraint on these columns was found".

**Why no `SOURCE_ASSERTED`.** The obvious candidate — `if (stock <
quantity) throw` before writing `stock - quantity` — establishes
`stock >= 0` for sequential executions of that code. Whether concurrent
executions must preserve it is still an interpretation of intent, which is
exactly what `HEURISTIC_HYPOTHESIS` means. Rather than introduce a
translation rule that would make the distinction thin, Phase 11 produces no
`SOURCE_ASSERTED` property; the category is defined for a later, stated
rule (assertion libraries, `invariant()` calls).

## Strengths

`database-integrity` (schema-enforced keys), `application-safety` (at most
one row/effect per key, declared business rules), `concurrency-relational`
(serial outcomes), `recovery-safety` (effects across crashes, retries,
redelivery, lost acknowledgments), `bounded-liveness` (no request fails —
checked as a final-state property within bounds; no eventual-delivery
requirement is turned into a terminal-state assertion: such properties,
like "a shipped order's event is eventually published", are not
synthesized at all).

## Templates

| template | instantiation |
| --- | --- |
| `atMostOne {table, key, where}` | for each actor's key values (parameters from the synthesized arguments; read results from the values the actor's reads returned in its solo run): `count(table where key ∧ where) ≤ 1` |
| `atLeast {table, groupBy, where, min}` | for each group present in the synthesized rows that satisfies it initially: `count(...) ≥ min` (groups that violate it initially are reported, not checked) |
| `nonNegative {table, column}` | for each initial row: `row.column ≥ 0` |
| `maxExternal {channel, max}` | `externals(channel) ≤ max`, × actors for per-request properties and distinct-key controls |
| `serialCell {table, column, key}` | `∨_k (actors ok = k ∧ cell ∈ V_k)` where `V_k` are the cell's values after every serial execution, in every order, of every subset of the actors in which `k` completed — computed by running the actors one after the other in the engine |
| `noFailedActors` | `actors failed = 0` |
| `formula` | as declared |

Each scenario invariant's description starts with its origin
(`[HEURISTIC_HYPOTHESIS] …`), so the origin also travels inside an emitted
scenario and into `commitrift model-check` reports.

## Reporting a result

| model result | property origin | finding |
| --- | --- | --- |
| counterexample | `HEURISTIC_HYPOTHESIS` | "MODEL_COUNTEREXAMPLE against a HEURISTIC_HYPOTHESIS … That the application requires this property has NOT been established; this is not a confirmed defect." |
| counterexample | other | "MODEL_COUNTEREXAMPLE: … violates `p` (ORIGIN). The property is enforced by the schema / declared by the user / implied by a declared integration contract" |
| none within bounds | any | "No violation found within the modeled bounds. This is not a proof that the application is safe." |

Hypothetical variants (a schema the application does not have,
`--hypothetical`) never decide a candidate's status. Runtime reproduction
of a counterexample is evidence about this schedule, input and database;
it does not establish the property as a requirement either.

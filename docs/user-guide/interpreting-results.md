# Interpreting results

Every CommitRift result carries the strongest evidence it has and no more.
This page says how to read each kind of result and what it does **not**
establish. Definitions: [evidence levels](../concepts/evidence-levels.md),
[property origins](../concepts/invariants.md#property-origins).

## The verification workflow

```text
TypeScript ──► check / trace ─────────────► static candidate (potential hazard)
          └──► discover ──► candidate + property (origin stated) + synthesized scenario
                                 │
                                 ▼
                         model-check (bounded) ──► model counterexample  |  none within bounds
                                 │                         │
                                 │       --validate-postgres ──► PostgreSQL-confirmed schedule (model SQL)
                                 │                         │
                                 │  --verify-source --allow-execution ──► original-code reproduced / diverged
                                 ▼
                         evidence report (text, JSON, HTML)
```

Each arrow is opt-in, and each step answers a narrower question:

| step | question it answers | question it does not answer |
| --- | --- | --- |
| `check` | is there a window between two independently durable or visible operations on some path? | whether the window matters, is repaired elsewhere, or is reachable |
| `discover` | which entries conflict on which resource, under which assumption? | whether the property is a requirement (unless declared or schema-enforced) |
| `model-check` | does some bounded schedule of the model violate the property? | whether the model matches PostgreSQL or the code |
| `--validate-postgres` | does PostgreSQL produce the same outcomes for that schedule of the model's SQL? | whether the application sends that SQL |
| `--verify-source` | does the original code, under that schedule, on a disposable database, violate the property? | whether the schedule occurs in production |

## Static findings (`check`)

* A diagnostic is a **potential** hazard derived from code structure. It is
  never marked reproduced.
* Confidence (`high`, `medium`, `low`) says how certain the path and the
  recognition are, not how likely a bug is.
* Common non-bugs found in the Phase 2 review: best-effort notifications,
  effects repaired by job retries or redelivery, two adjacent but unrelated
  operations, test and seed scripts. Declare recovery
  ([configuration](../reference/configuration.md#recovery-declarations)),
  baseline, or exclude them.
* `CP9001` is not a finding: it marks code the analysis did not understand
  next to recognized effects.

## Discovery candidates

Read the status, then the property origin:

| status × origin | reading |
| --- | --- |
| `MODEL_COUNTEREXAMPLE`, `SCHEMA_ENFORCED` / `USER_DECLARED` / `FRAMEWORK_CONTRACT` | a model schedule violates a property the schema, you, or a declared contract require |
| `MODEL_COUNTEREXAMPLE`, `HEURISTIC_HYPOTHESIS` | a model schedule violates a property the code *suggests*; not a confirmed defect |
| `NO_VIOLATION_WITHIN_BOUNDS` | none within these bounds and this synthesized state; check the `distinct-key` control and the bounds |
| `INCONCLUSIVE`, `UNSUPPORTED`, `BLOCKED` | not decided; the reason names the blocking construct or missing declaration |

Concurrency of two invocations is itself an assumption (declared,
framework, or `discovery-default`), printed with the candidate.

## Model-check verdicts

* `COUNTEREXAMPLE_FOUND`: a possible execution of the **model** under the
  scenario's actors, data, isolation level and bounds. The minimized
  witness has been replayed in the model.
* `NO_COUNTEREXAMPLE_WITHIN_BOUNDS`: covers the declared actors, arguments,
  initial rows, isolation level, faults and bounds — nothing else. Not a
  proof of race freedom.
* SERIALIZABLE is a conservative serialization-graph abstraction: a
  SERIALIZABLE counterexample may not be realizable on PostgreSQL
  ([PostgreSQL model](../architecture/postgres-model.md#isolation)).
* A replica counterexample cannot go beyond `model-counterexample`: no
  streaming replication is run.

* Phase 15: a discovery verdict is relative to the **initial worlds** it
  explored. Read the candidate's coverage section: how many worlds, which
  input relationships, whether each world's schedule search completed, and
  what was not explored. `domainExhaustive` is never true for synthesized
  inputs ([verdict semantics](../phase15/VERDICT-SEMANTICS.md)).

## Runtime results

* `postgres-confirmed-schedule`: PostgreSQL agreed with every predicted
  statement outcome and the final rows for the model's SQL. If the
  invariant reads actor outcomes or external effects, those come from the
  model's application logic.
* `original-code-reproduced` (and the crash, in-flight and connection-loss
  variants): the original code followed the schedule and violated the
  property, on a disposable database, with the harness's inputs. A run that
  violates it along a *different* schedule is `diverged`, never a
  reproduction.
* A probe that holds (`original-code-recovery-reproduced`,
  `original-code-invariant-held`) is bounded evidence about one schedule.

## What no result says

* That an application is correct.
* That a schedule happens in production, or how often.
* That a heuristic property is a requirement.
* That a first-party fixture result transfers to another application.
* That any finding was confirmed by an independent human reviewer: **no
  independent human review has been performed** for any phase
  ([evaluation methodology](../research/evaluation-methodology.md)).

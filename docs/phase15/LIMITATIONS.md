# Phase 15 limitations

The current, complete list is `docs/reference/limitations.md` (updated
with a "Phase 15 changes" section). What Phase 15 adds or leaves:

## Input and initial-state coverage

* **Not exhaustive.** Worlds cover relationship classes (null/undefined
  where annotated, branch operands and boundaries, stored = / ≠ written
  value, stored guard boundaries, non-key arguments differing between
  actors), one relationship per world. Value domains are never
  enumerated; combinations of relationships are not explored;
  `domainExhaustive` is always false.
* **Nullability needs an annotation.** Without a TypeScript annotation a
  path's nullability is unknown and `null` is not tried (listed as
  unexplored). zod `nullable`/`optional` do not yet feed domains.
* **Branch coverage is from solo runs.** Reached branch outcomes are those
  an actor takes running alone on an explored world's rows; concurrent
  schedules may take others. It orders worlds; it is not a correctness
  measure.
* **Two actors.** Candidates keep the pair of actors detection chose;
  "more actors than the candidate's pair" is always listed as unexplored.
* **Controls and hypothetical variants** keep one world.
* **Stored boundary values** come from integer comparisons in guards over
  read rows; non-integer comparisons (time, text order) are not varied.

## Synthesis

* Guard satisfaction handles comparisons of a read row's property or an
  argument path with a literal, integer orderings, and `includes` of a
  literal; other guards (calls into packages, computed comparisons) still
  stop synthesis, now with the location and failure named.
* An `invalid` world means no valid state was found **within the bounds**;
  it never proves that none exists.
* Literal-union aliases are followed through relative imports and
  re-exports only (not `paths` aliases or packages).

## Model fidelity

* `NULLS NOT DISTINCT` unique rules over nullable columns are not enforced
  by the model; a counterexample on such a table is INCONCLUSIVE (verified
  necessary on PostgreSQL: the original code does not reproduce it).
* Properties over generated identifiers (serial outcomes that are
  generated values) still cannot be stated (midday).
* Replica models are model-only (the lockstep oracle has one server): the
  new read-your-writes counterexamples are not PostgreSQL-confirmed.

## Source support still missing (blocks real candidates)

zod `strictObject` (school-ledger's first blocker), zod string formats and
`nullable`/`optional`/`default`, `for`/`while` loops (retry helpers),
`await req.json()`, `Response.json`, `Date#getTime()`, JSON projections in
`select`, predicates hoisted into locals, multi-row `FOR UPDATE`.

## Evaluation

No independent human review. The Phase 15 set was authored by an AI agent
that knew the documented subset and Phase 14's notes; only its first
scoring is held out; every other number in `PRECISION-RECALL.md` is
post-feedback or a regression reference.

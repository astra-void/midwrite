# `midwrite discover`: candidates and scenarios from source

`discover` finds concurrency and failure candidates in the original
TypeScript without a hand-written scenario. It indexes entries and their
database and external effects, pairs effects that may interact, attaches
property candidates with a stated origin, synthesizes small valid initial
states, writes ordinary `model-check` scenarios and, with `--model-check`,
explores them within budgets. It never executes application code.

It handles generated keys, multi-module schemas, repositories, foreign
keys, return values and the response step, read replicas, upserts,
row-lock strengths, zod validation, bounded loops and request bodies.
Internals: [scenario synthesis](../architecture/scenario-synthesis.md) and
[world search](../architecture/world-search.md). All flags:
[CLI reference](../reference/cli.md#discover).

## Typical use

```bash
midwrite discover .                                     # static index + candidates + synthesized scenarios
midwrite discover . --model-check                       # also explore them (bounded)
midwrite discover . --model-check --format html -o discovery.html
midwrite discover . --emit-scenarios ./midwrite-scenarios
midwrite discover . --contracts midwrite-contracts.json --entry src/app/checkout
midwrite discover . --cache .midwrite-cache.json      # cache extractions between runs
```

Exit status: `1` when at least one model counterexample was found, `0`
otherwise (errors: `2`).

## Detector families

| class | what it looks for | property (origin unless declared) |
| --- | --- | --- |
| `check-then-act` | a read with equality conjuncts, then an insert into the same table decided by it, sharing a key column | at most one row per key (`HEURISTIC_HYPOTHESIS`; `SCHEMA_ENFORCED` when a non-partial unique set covers the key) |
| `status-transition` | read a row, test its state, unconditional `UPDATE` to another state, consequences under the same guard | at most one consequence / effect per transition |
| `read-modify-write` | `UPDATE` of a column computed from a value read earlier | the cell equals some serial execution |
| `write-skew` | a predicate read deciding a write of another row of the group | **declared only**; otherwise `BLOCKED` |
| `duplicate-processing` | declared at-least-once delivery with external effects, or with inserts a declared invariant covers | at most one effect per message (`FRAMEWORK_CONTRACT`); declared invariants on the inserted tables |
| `external-effect-gap` | external effect followed by writes, with a declared retry (or inside a transaction the code re-runs) | at most one effective effect per request |
| `ambiguous-commit` | an autocommit write in `try` repeated in its `catch`, or a write in a transaction whose COMMIT is in the `try` (a retry wrapper's callback) | at most one row per key |
| `lock-order` | two row locks on one table in one transaction keyed by different parameters | no request fails (bounded liveness) |
| `resubmitted-write` | an entry with a declared crash retry whose `INSERT` (single or multi-row) can commit before the crash | only with a declared invariant on the table or a covering unique constraint; otherwise a recorded rejection |
| `parent-child` | a read finds a parent row by a unique key and decides an `INSERT` of a child holding that key, while another entry deletes the parent | no orphan child: `SCHEMA_ENFORCED` with a `NO ACTION` foreign key (a 23503 is not an application defect), `HEURISTIC_HYPOTHESIS` without one |
| `return-value` | an entry named by a **declared** property over what callers are told (`atMostOneSuccess`, `exactlyOneSuccess`, `everySuccessReturns`, or a formula reading `returned`) | the declared property only; never inferred |

Rules, rejections and pairing:
[scenario synthesis § detectors](../architecture/scenario-synthesis.md#detectors).
Only `FOR UPDATE` and `FOR NO KEY UPDATE` count as a protection (two
`FOR SHARE` readers do not exclude each other), and actors whose caller
retries after a crash get the response step.

## Reading a candidate

Each candidate reports its entries (and how each is invoked), the
conflicting operations with locations, the shared resource, every property
with its origin and rationale, recognized protections, assumptions,
unsupported operations on the path, and the scenarios built for it
(`actual`, a `distinct-key` control, and with `--hypothetical` a schema the
application does not have). A real excerpt is in
[first verification](../getting-started/first-verification.md#2-discover-candidates-without-writing-a-scenario).

| status | meaning |
| --- | --- |
| `MODEL_COUNTEREXAMPLE` | the `actual` scenario has a counterexample that replays in the model; interpret it by the property's origin |
| `NO_VIOLATION_WITHIN_BOUNDS` | no counterexample within the scenario's bounds; not a proof |
| `STATIC_CANDIDATE` | a scenario was built but not model-checked (no `--model-check`, or beyond `--max-candidates`) |
| `INCONCLUSIVE` | no valid initial state could be synthesized, or a budget ran out |
| `UNSUPPORTED` | an actor reaches an operation the model does not represent; the reason and location are printed |
| `BLOCKED` | the property must be declared (for example write skew) |

A counterexample against a `HEURISTIC_HYPOTHESIS` is reported as such and
is never a confirmed defect ([invariants](../concepts/invariants.md)).
Concurrency itself is an assumption: declared, `framework` (Server Actions,
route handlers), or `discovery-default` for any other exported function.

## Declaring what source cannot say

Retries, at-least-once delivery, external idempotency, business invariants,
argument values, read-replica factories and harness inputs go into an
optional `midwrite-contracts.json`
([contracts guide](contracts.md)). Everything taken from it is reported as
declared.

## From a candidate to runtime evidence

Emitted scenarios carry a `sourceVerification` section when the contracts
map the entry to an authorized harness. Running them needs the separate,
explicit `model-check --verify-source --allow-execution`
([source verification](source-verification.md)); `discover` itself never
runs code.

Every candidate ends with a **next step** (`nextStep` in JSON, "Next
step:" in text and HTML): a BLOCKED or INCONCLUSIVE candidate prints a
declaration skeleton, a counterexample the exact `model-check` command for
the `--emit-scenarios` directory (and labels the `--verify-source
--allow-execution` command as executing application code). A synthesized
scenario is checked to re-extract exactly what discovery extracted (round
trip) before it is model-checked; discovery extracts with JavaScript value
semantics and opts its scenarios into the `js-values`, `timestamps` and
`json` features when they need them.

## Input exploration

With `--model-check`, each actual variant is checked in up to
`--max-worlds` (default 8) **initial worlds**: the synthesized one, then
worlds that each change one input relationship — a nullable argument set
to `null` (only where the TypeScript annotation admits it), an argument at
a branch's operand or boundary, a stored value distinct from (or equal to)
the value an actor writes, a stored value at the boundary of a guard, a
non-key argument that differs between the actors. The search stops at the
first validated counterexample (`--all-worlds` continues). Guards that
synthesized inputs failed (an admin role, a caller kind) are satisfied
with **stated** preconditions ("SYNTHESIZED precondition … not assumed of
every caller").

Every model-checked candidate has a **coverage** section, kept in four
dimensions — source semantics, initial states, actor inputs, schedules —
plus branch outcomes and the list of worlds and gaps
(`--explain-coverage` prints them). A `NO_VIOLATION_WITHIN_BOUNDS` result
always says how many worlds were explored and that the input domain was
not exhaustively explored. Details:
[world search](../architecture/world-search.md).

```text
  Input exploration (relationship-search): 3 world(s) model-checked, controls included — actual configuration: 1 distinct row set(s), 2 distinct argument vector(s)
    world `base` (actual, Base): explored — NO_COUNTEREXAMPLE_WITHIN_BOUNDS
    world `null:parentId` (actual, Domain): explored — COUNTEREXAMPLE_FOUND
  Schedule exploration (sleep-sets): completed for 2 world(s), truncated for 0, unsupported for 0
  Branch outcomes reached alone: 5 of 8
  Input domain exhaustive: no (every derived relationship class was explored)
  Status: MODEL_COUNTEREXAMPLE
  In the initial world `null:parentId` (parentId = null for every actor): MODEL_COUNTEREXAMPLE: …
```

## Known blind spots

Measured on the frozen evaluation sets
([evaluation datasets](../research/evaluation-datasets.md); numbers in the
[evidence index](../research/evidence-index.md)), precision has stayed at
1.000 while recall is low (one third of the positives on the newest set).
The most frequent causes of misses:

* JavaScript standard-library calls (`Set.prototype.has`,
  `Number.isSafeInteger`, `Math.min`): the candidate is `UNSUPPORTED`;
* status transitions whose write is keyed by the id of a row read first
  (no candidate is detected);
* on real projects: zod refinements (`.superRefine()`), optional chains on
  call results, `Promise.all` with a used result, multi-row invariants
  nobody declared (write skew stays `BLOCKED`);
* computed keys, time arithmetic, JSON projections and comparisons, a stale
  replica read without a declared property, and return-value properties
  without a declaration.

On the real corpus no candidate has been model-checked to a counterexample
except saas-starter's heuristic one (never executed). See
[limitations](../reference/limitations.md) and
[corpus evaluation](../development/corpus-evaluation.md#current-blockers).

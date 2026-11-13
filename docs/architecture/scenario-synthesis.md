# Candidate discovery and scenario synthesis

Crate: `midwrite-discover` (with `midwrite-analyzer::entries` and the
model checker's own loader and engine). Command: `midwrite discover`
([user guide](../user-guide/discover.md)). This page describes the
pipeline, its rules and the representations it reports.

```text
project root
  │ entries.rs: entry candidates (Server Actions, route handlers, exported functions,
  │             wrapped callbacks, class methods, object properties) as source selectors
  ▼
probe scenario ─► table bindings (schema modules, generated columns, foreign keys)
  ▼
extract each entry with the same ExtractRequest a generated scenario will use
  ▼
effects.rs: effect summaries — every value traced to a parameter path, literal,
            read column or computation; (table, column) resource index
  ▼
detect.rs: detectors per entry ─► pairing through the resource index (≤ --max-partners)
           ─► feasibility filtering (reachability, literal filters, declared non-concurrency)
  ▼
property candidates with origin (schema, contracts, framework contract, hypothesis)
  ▼
synth.rs: arguments from parameter paths and typed input domains; rows by running each
          actor alone in the engine and satisfying the reads and guards that stopped it;
          keys, defaults, foreign keys; serial outcomes computed by the engine; variants
          (actual, distinct-key control, distinct-request-key, hypothetical)
  ▼
scenario (ordinary model-check format) ─► optional bounded model checking (budgets),
          over several initial worlds per actual variant (world search)
  ▼
counterexample validation (replay, violation, operations exercised) ─► coverage ─► ranking, deduplication
  ▼
discovery report (text / JSON `midwrite.discovery` / HTML); --emit-scenarios writes the
scenarios and a reproduction artifact per counterexample
```

## Principles

1. **No second semantics.** Discovery never lowers TypeScript and never
   evaluates SQL itself. Entries are extracted by the request the
   scenario loader makes; rows are found by the model's engine; scenarios
   are loaded and validated by the scenario loader; verdicts come from
   `report::check`. An emitted scenario is an ordinary scenario file:
   `midwrite model-check` gives the same verdict and the same state count
   (tested in `crates/midwrite-cli/tests/discover_commerce.rs`).
2. **Static by default, budgeted model checking, separately authorized
   execution.** `discover` never executes application code
   (`executesApplicationCode: false` in every report). Running original
   code stays a separate command: `model-check <emitted scenario>
   --verify-source --allow-execution`.
3. **Provenance before precision.** Every property carries its origin
   (`SCHEMA_ENFORCED`, `SOURCE_ASSERTED`, `USER_DECLARED`,
   `FRAMEWORK_CONTRACT`, `HEURISTIC_HYPOTHESIS`) and every assumption its
   origin (`source-derived`, `declared`, `framework`, `discovery-default`);
   no later step changes them.
4. **Unsupported is visible.** Unknown operations of every entry are
   listed and categorized; a candidate with an unknown operation on its
   path is never reported safe; refused tables, failed extractions and
   unsatisfiable states are reported with the reason.

What comes from source and what must be declared:

| from source (static) | declared (`midwrite-contracts.json`, [contracts](../user-guide/contracts.md)) |
| --- | --- |
| entries and their syntactic kind | whether an exported function is called concurrently (`concurrent`) |
| wrapper invocation and guards | the invocation of a wrapper whose source is unknown (`invocation`, `preconditions`) |
| database effects, predicates, keys, transactions, locks | which calls are external effects (`externals`) and whether a channel deduplicates (`dedup`) |
| transaction helpers without a loop (one attempt) | helpers with a retry loop (`transactionHelpers` with `attempts`) |
| retry-after-failure code (`try`/`catch` repeating a write), bounded retry loops | at-least-once delivery (`delivery`), client retries after a crash (`retry`) |
| unique constraints, keys, foreign keys, defaults | business invariants (`invariants`), return-value properties |
| | values validation code would accept (`values`), harness inputs (`harness`), replica factories |

## Entry index

`midwrite-analyzer/src/entries.rs` lists, for every module, the functions
a caller outside the module can reach, each as a source selector:

| kind | selector | example |
| --- | --- | --- |
| `server-action` | `file#name`, `file#name/arg[i]` | exports of a `"use server"` module, callbacks of the call that initializes them |
| `route-handler` | `file#GET` … | `GET`/`POST`/`PUT`/`PATCH`/`DELETE`/`HEAD`/`OPTIONS` of a `route.ts` |
| `exported-function` | `file#name` | any other exported function |
| `wrapped-callback` | `file#name/arg[i]` | a callback passed to the call that initializes an export |
| `class-method` | `file#Class/method[m]`, `/static[m]` | methods of an exported class |
| `object-property` | `file#obj/property[p]` | function-valued properties of an exported object |
| `trpc-procedure` | `file#router/arg[0]/property[p]/arg[0]`, `file#name/arg[0]` | the inline resolver of a tRPC procedure (`procedure.input(s).mutation(fn)`, `.query`, `.subscription`) in a router object or exported on its own; with a `trpc` declaration for its builder in `midwrite-contracts.json` it is extracted with the declared context and its `.input(schema)` parse ([contracts](../user-guide/contracts.md)); without one it is reported as not extractable, with that reason |

`export default`, export lists and re-exports are recorded as skipped with
the reason. "Exported" does not mean "externally callable": how
concurrency is established is an assumption of the candidate (below). The
index also records each module's project imports, whether it is a schema
module, and **transaction helpers** (functions that pass a parameter as the
callback of `<receiver>.transaction(...)`, with the options parameter and
whether the body loops). An exported function another analyzed entry
inlines (a repository method) is a helper for pairing, not a partner: its
effects already appear in its callers.

## Extraction

Entries whose import closure mentions neither a database receiver nor a
declared external callee (a word match over the files, a superset of what
the extractor can lower) are listed as `not-extracted`. The rest are
extracted with the loader's own request: the table bindings of a probe
scenario that reads every table of the schema module(s), the declared
externals, the transaction helpers (source-derived when they have no loop:
one attempt; declared otherwise), inline depth 3, and the declared
contract — or, without one, an empty contract that makes the extractor
report a wrapper's facts instead of refusing. One `ProjectContext` (module
index, schema, resolution) is shared by all entries of a run.

## Effect summaries

`effects.rs` restates the extracted code vectors; it adds nothing the model
does not execute.

* **Database effects** in code order: access (`Read`, `Count`, `Insert`,
  `Update`, `Delete`), table, connection (`pool` autocommit or `tx`), row
  lock, transaction, the predicate split into `column = value` conjuncts and
  other conjuncts, written columns, `ON CONFLICT`, joined tables, the result
  local and whether a branch reads it, and whether it lies in a `try` block
  or a `catch` handler.
* **Value provenance** (`Val`): `Param{path}`, `Lit`, `Read{effect,
  column}`, `ReadRow`, `ReadCount` (`rows.length`, `count()`, `returning()`
  rows), `ColumnRelative` (an expression over the row being written,
  computed atomically by the database), `Computed`, `Unknown`. Locals are
  traced through `Let` chains, including the return slots inlining creates.
* **External effects** with channel and key provenance; **guards** (branches
  whose condition reads a database result, and the columns compared with a
  literal); **control flow** per code vector, so "B can follow A" and "this
  guard decides whether B runs" are computed; transactions with isolation
  and retry count; unknown operations with location.

### Resource identity

| resource | identity | from |
| --- | --- | --- |
| `unique-key` | table + key columns (+ literal filter) | a check's equality conjuncts matched with the insert's written values |
| `row` | table + the read's equality key | status transitions, lock order |
| `row-predicate` | table + predicate | write skew |
| `column` | `table.column` + row key | read-modify-write |
| `external-channel` | channel name | declared externals |

Two values are the same logical value when their provenance is equal after
resolving `Read{e, c}` through the read's own equality (`read#0.id` with
`id = $input.productId` is `$input.productId`). Unknown and computed values
are never the same value. The resource index maps `(table, written column)`
to the entries that write it; pairing never enumerates all pairs of
functions.

## Detectors

Each detector is a rule over one entry's effect summary
(`detect.rs`); the families and their properties are listed in the
[user guide](../user-guide/discover.md#detector-families). A detector
**records** a recognized protection (a unique constraint, a row lock of
strength `FOR UPDATE` / `FOR NO KEY UPDATE`, a conditional update, a
deduplicating channel) on the candidate instead of dropping it, so the
model checker decides whether the protection works. It **rejects** a
combination only for a reason it writes into `rejected`, with the location:
a check and an insert without an argument-dependent key in common; a
transition whose conditional update's row count is checked; a repeated
transition without observable consequence; an external effect followed by
writes without a declared retry ("a crash alone repeats nothing"); a partner
whose inserted literal differs from the check's literal filter (provably
distinct rows); entries declared non-concurrent. Declared invariants replace
hypotheses on the same table.

**Concurrency is an assumption.** Two invocations overlapping is declared
(`concurrent`, `delivery`), `framework` for Server Actions and route
handlers (double submit, two tabs, client retry), or `discovery-default` for
any other exported function ("NOT established from source"). It is part of
every multi-actor candidate.

**Pairing.** For check-then-act and read-modify-write candidates, partners
are the other entries that write the same `(table, column)`, excluding
inlined helpers, each unordered pair once, at most `--max-partners`
(default 6; the rest counted as `partnersTruncated`).

**Feasibility filtering** uses only the candidate's own facts: reachability
on the control-flow graph (mutually exclusive branches never form a check
and an act), literal filters, and declared non-concurrency. Whether the
schedule exists or a protection holds is left to the model on a
synthesized state.

## Property candidates

Application requirements cannot in general be read from code. Every
candidate carries one or more property candidates with an origin
([invariants § property origins](../concepts/invariants.md#property-origins)):
`SCHEMA_ENFORCED` only when a non-partial unique set (primary key,
`.unique()`, `unique().on`, `uniqueIndex().on` without `.where`) covers the
checked key; a partial unique index is a protection, never a global
constraint. A missing constraint is not evidence that uniqueness was
intended; the rationale of a check-then-act hypothesis says so.
`SOURCE_ASSERTED` is defined but no rule produces it: a guard such as
`if (stock < quantity) throw` establishes `stock >= 0` for sequential
executions only, and whether concurrent executions must preserve it is an
interpretation of intent — what `HEURISTIC_HYPOTHESIS` means. Recovery
requirements ("an event is eventually published") are never synthesized.

Strengths: `database-integrity`, `application-safety`,
`concurrency-relational` (serial outcomes), `recovery-safety`,
`bounded-liveness` (no request fails, checked as a final-state property
within bounds).

| template | instantiation |
| --- | --- |
| `atMostOne {table, key, where}` | for each actor's key values (from synthesized arguments, or from the values its reads returned in its solo run): `count(table where key ∧ where) ≤ 1` |
| `atLeast {table, groupBy, where, min}` | for each group of the synthesized rows that satisfies it initially (groups violating it initially are reported, not checked) |
| `nonNegative {table, column}` | for each initial row: `row.column ≥ 0` |
| `maxExternal {channel, max}` | `externals(channel) ≤ max`, × actors for per-request properties and distinct-key controls |
| `serialCell {table, column, key}` | `∨_k (actors ok = k ∧ cell ∈ V_k)`, where `V_k` are the cell's values after every serial execution, in every order, of every subset of actors in which `k` completed — computed by running the actors one after the other in the engine |
| `noFailedActors` | `actors failed = 0` |
| return-value templates, `formula` | as declared ([contracts](../user-guide/contracts.md)) |

Each scenario invariant's description starts with its origin
(`[HEURISTIC_HYPOTHESIS] …`), so the origin travels into emitted scenarios
and `model-check` reports. Hypothetical variants never decide a status.

## State synthesis

`synth.rs` builds an ordinary scenario file (`schemaVersion` 1) with tables
read `from` the Drizzle schema, entries selected by selector, synthesized
arguments and rows, invariants with their origin, and bounds.

* **Tables**: every table an actor reads, writes or joins, the property's
  table, and the tables their foreign keys reference transitively, ordered
  parents first. Tables may come from several schema modules
  (`module#binding`).
* **Arguments**: from the parameter paths the code reads, typed by their
  [input domain](world-search.md#typed-input-domains) and by the columns
  they meet — key-like integers get distinct small integers in path order,
  `uuid` paths valid UUIDs, string-literal unions their first member, time
  and JSON columns canonical values; declared `arguments` and `values`
  replace them.
* **Rows: running each actor alone.** Rows are not invented from templates.
  Each actor runs alone in the model's engine from the rows found so far:
  (1) pre-seeding — an object argument whose fields are the columns of
  exactly one table including its primary key is taken to be that row, and
  rows a candidate's UPDATE/DELETE targets are seeded (each stated as a
  note); (2) a loop of at most 16 iterations: the last read that came back
  empty on the actor's path is satisfied by a row built from its *closed*
  predicate (comparisons with literals; contradictions refused), joined
  reads get their missing rows, count reads get rows until the guard passes,
  a guard over a read row or an argument is satisfied with a stated
  precondition ([world search § guards](world-search.md#guards-and-stated-preconditions));
  a read the candidate needs empty — the check of a check-then-insert — is
  never satisfied; (3) completion of every added row: keys, constant
  defaults, sequences, not-null columns, parent rows for foreign keys (depth
  6), unique sets; (4) parents of rows the actor inserts; (5) a final round
  in which every actor must still reach its operations alone. Rows whose
  keys the execution generates are never seeded: a generated key's identity
  is its generation event.
* The result is `satisfied`, `unsatisfiable` (with the reason and where the
  actor stopped) or `unsupported` (with the location). The loader then
  checks not-null, CHECK, primary and unique keys; discovery checks the
  source's foreign keys. A synthesized state is valid only with respect to
  the modeled constraints: triggers, row-level security and constraints the
  extractor does not read are not modeled.

### Variants and bounds

| variant | kind | when |
| --- | --- | --- |
| `actual` | actual | always: the application as analyzed |
| `distinct-key` | control | multi-actor check-then-act, read-modify-write, status transition; varies the parameters behind read keys too |
| `distinct-request-key` | actual (assumed client behavior, stated) | multi-actor candidates whose effects send an argument as idempotency key |
| `hypothetical-unique` | hypothetical | `--hypothetical`, check-then-act without a covering unique set |

Only `actual` variants decide a candidate's status (the first with a
counterexample, else the first). Actor relations: `same`, `distinct-key`,
`distinct-row` (write skew), `swapped` (lock order), `distinct-request-key`.
A scenario gets the `foreign-keys` feature when its tables carry foreign
keys, `replicas` and the declared factories when an actor reads through a
replica, `response: true` for actors whose caller retries after a crash,
and `js-values`, `timestamps`, `json` when it needs them.

`maxSteps = max(64, 3 × operations × (1 + retries))`; `maxStates`,
`maxWallMs`, `maxMemoryBytes` per scenario from the flags; `maxRetries`
raised to the code's transaction-helper retries (at most 2); crashes and
in-flight losses from the candidate's environment.

Scenario files are written with sorted keys, named
`<candidate-id>-<variant>.json`, with the project root relative to the
output directory; two runs produce identical files. Each synthesized
scenario is re-extracted and must equal what discovery extracted (round
trip) before it is model-checked. With harness mappings in the contracts a
`sourceVerification` section is added.

## Model checking and counterexample validation

Each actual variant is checked over up to `--max-worlds` initial worlds
([world search](world-search.md)). For every counterexample
(`pipeline.rs`):

1. it replays strictly in the model (`witness::replay`);
2. the replayed, completed schedule violates the named invariant;
3. the initial state passed the loader and the source foreign keys;
4. which of the candidate's operations it executes is recorded;
5. it is minimized by the model checker;
6. its assumptions travel with the candidate and the emitted scenario.

A counterexample that fails 1 or 2 makes the candidate `INCONCLUSIVE`. A
counterexample on a table with a uniqueness rule the model does not
enforce (a partial unique index, or a `NULLS NOT DISTINCT` rule with a
column the model does not model) is `INCONCLUSIVE` with the rule named,
never a finding (`pipeline.rs::unenforced_unique`).
Every candidate's text ends with a **next step** (`nextStep`).

## Identity, ranking and deduplication

The candidate id is `CR-SYNTH-` plus the first six hex digits of a stable
fingerprint (class, actors' selectors, resource, property templates with
origins, environment, conflict roles and operations; no line numbers).
Candidates sharing class, resource and first-entry operations form a group;
later members get `duplicateOf`. Candidates that differ in isolation,
logical key, failure mechanism, assumptions, location, or actual versus
hypothetical schema have different fingerprints.

Ranking is deterministic and explained (`ranking` lists the factors), with
no numeric confidence: status (`MODEL_COUNTEREXAMPLE`, `STATIC_CANDIDATE`,
`INCONCLUSIVE`, `NO_VIOLATION_WITHIN_BOUNDS`, `UNSUPPORTED`, `BLOCKED`),
strongest property origin, no unsupported operation on the path, exposure
(request handlers and declared entries first), fewer entries and
assumptions, fingerprint.

## Budgets

`--max-candidates` (64), `--max-partners` (6), `--max-worlds` (8 per
actual variant), `--max-states` (200 000 per scenario), `--max-wall-ms`
(60 000 per scenario), `--max-memory-bytes`, `--max-total-states`
(5 000 000). Candidates, worlds and scenarios beyond a budget are counted
and listed, never silently dropped.

## Caching

`--cache FILE` caches **extractions** only, under a key covering
everything the extractor can read: tool version and cache format, the
selector, the contract, every file of the entry's import closure, the
project's `tsconfig.json`/`package.json`/`midwrite.json`, the table
bindings, the external and transaction-helper bindings, and the database
receivers. A changed helper invalidates exactly its importers; a schema or
binding change invalidates everything; a channel's deduplication (not read
by extraction) invalidates nothing. Candidates, worlds, invariants,
verdicts and coverage are recomputed on every run, so a change of
contracts, input domains, budgets or properties never reuses a stale
result.

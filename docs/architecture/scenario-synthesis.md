# Candidate discovery and scenario synthesis

Crate: `commitrift-discover` (with `commitrift-analyzer::entries` and the
model checker's own loader and engine). Command: `commitrift discover`.
Design record: [Phase 11 architecture](../phase11/ARCHITECTURE.md); this
page summarizes the current pipeline.

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
synth.rs: arguments from parameter paths; rows by running each actor alone in the
          engine and satisfying the reads that came back empty; keys, defaults, foreign keys;
          serial outcomes computed by the engine; variants (actual, distinct-key control,
          distinct-request-key, hypothetical)
  ▼
scenario (ordinary model-check format) ─► optional bounded model checking (budgets)
  ▼
counterexample validation (replay, violation, operations exercised) ─► ranking, deduplication
  ▼
discovery report (text / JSON `commitrift.discovery` / HTML); --emit-scenarios writes the scenarios
```

## Stages and their documents

| stage | rule of thumb | document |
| --- | --- | --- |
| entry and effect index | entries are source selectors; extraction requests equal the loader's | [Phase 11 effect index](../phase11/EFFECT-INDEX.md) |
| detectors | each records protections (unique constraint, row lock, conditional update, deduplicating channel) instead of dropping the candidate; rejections are written with their reason | [Phase 11 candidate discovery](../phase11/CANDIDATE-DISCOVERY.md) |
| properties | origin never upgraded; templates instantiated with synthesized values | [Phase 11 invariant synthesis](../phase11/INVARIANT-SYNTHESIS.md) |
| state synthesis | only closed predicates of comparisons with literals are satisfied; values validation code would reject must be declared | [Phase 11 state synthesis](../phase11/STATE-SYNTHESIS.md) |
| model checking | `report::check`, sleep sets by default, budgets per scenario and in total | [Phase 11 source-model fidelity](../phase11/SOURCE-MODEL-FIDELITY.md) |
| contracts | what source cannot say, always reported as declared | [contracts guide](../user-guide/contracts.md) |

## Changes after Phase 11

* **Phase 12**: generated keys modeled as values whose identity is the
  generation event; synthesis never seeds rows whose keys the execution
  generates; `uuid` arguments are valid UUIDs; multi-module schemas
  (`module#binding`); distinct-key controls vary the parameters behind
  read keys; the `resubmitted-write` family; a shared `ProjectContext` and
  an incremental cache ([Phase 12 architecture](../phase12/ARCHITECTURE.md)).
* **Phase 13**: a synthesized scenario gets the `foreign-keys` feature when
  its tables carry foreign keys (the table closure includes referenced
  tables), `replicas` and the declared `replicaFactories` when an actor
  reads through a replica, and `response: true` for actors whose caller
  retries after a crash; only `FOR UPDATE` / `FOR NO KEY UPDATE` count as
  a protection; the Phase 12 `foreign-key-locks` limitation note is no
  longer emitted ([Phase 13 architecture § 3](../phase13/ARCHITECTURE.md#3-versioned-semantics-track-g2)).

## Identity, ranking and deduplication

The candidate id is `CR-SYNTH-` plus the first six hex digits of a stable
fingerprint (class, actors' selectors, resource, property templates with
origins, environment, conflict roles and operations; no line numbers).
Candidates sharing class, resource and first-entry operations form a group;
later members get `duplicateOf`. Ranking is deterministic and explained:
status, strongest property origin, no unsupported operation on the path,
exposure, fewer entries and assumptions, fingerprint
([Phase 11 § ranking](../phase11/CANDIDATE-DISCOVERY.md#ranking)).

## Budgets

`--max-candidates` (64), `--max-partners` (6), `--max-states` (200 000 per
scenario), `--max-wall-ms` (60 000 per scenario), `--max-memory-bytes`,
`--max-total-states` (5 000 000). Candidates and scenarios beyond a budget
are counted, never silently dropped.

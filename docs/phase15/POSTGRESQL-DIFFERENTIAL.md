# PostgreSQL differential results and search-algorithm agreement

Raw: `reports/phase15/evidence/` (`scripts/phase15-evidence.py`,
PostgreSQL 17.11, disposable clusters).

## Every generated world under four search algorithms

`discover --model-check --emit-scenarios` on four first-party applications
(`fixtures/phase13/eval-app`, `fixtures/phase14/eval-app`,
`fixtures/phase15/eval-app`, `fixtures/phase15/worlds-app`) wrote **98
scenarios** (one per explored world and variant). Each was model-checked
with the unreduced search (`--no-por`), sleep sets (default), classic DPOR
(`--dpor`) and source-set DPOR (`--source-dpor`):

**98 of 98 agree on verdict and completeness.** For a fixed world the
reductions preserve the verdict, as on the Phase 10–14 benchmarks; new
worlds change the inputs, not the algorithms. A CLI test repeats this on
every world of the worlds application
(`the_reductions_agree_with_the_unreduced_search_on_every_world`).

## Lockstep replay on PostgreSQL

Every scenario with a counterexample (31) was replayed on PostgreSQL in
lockstep (`--validate-postgres`): **28 confirmed, 166 database outcomes
compared, 0 disagreements**. The 3 others were not replayed: their models
read through a replica, and the lockstep oracle has one PostgreSQL server
("a model with a read replica is not replayed", a Phase 13 limitation):
the two read-your-writes counterexamples found by the new
`stored-distinct` worlds (P14-E21, P15-E01) and one Phase 14 set replica
case. Their evidence stays `model-counterexample`.

## Original code

`fixtures/phase15/worlds-app` (first-party, authorized harness
`commitrift-source.json`, Drizzle on node-postgres), `--verify-source
--allow-execution --source-repeat 3`:

| entry | candidate | world | original code | runs |
| --- | --- | --- | --- | --- |
| `createFolder` | MODEL_COUNTEREXAMPLE (declared `one-folder-per-place`) | `null:parentId` | **reproduced** (`original-code-reproduced`): two root folders with one name | 3/3 |
| `startExport` | MODEL_COUNTEREXAMPLE (declared `export-quota-non-negative`) | `stored-boundary:quotas.remaining=1` | **reproduced**: the quota ends at −1 | 3/3 |
| `createLabel` (safe: NULLS NOT DISTINCT) | INCONCLUSIVE (unenforced rule) | base | **not reproduced**: PostgreSQL rejects the duplicate; statements diverge at the second INSERT | 0/3 |

In both reproduced runs every statement, table and outcome agreed with the
model. The base worlds of both defects have no counterexample (nothing to
replay): each defect needs the world the search generated.

The `createLabel` row is the differential evidence for the Phase 15 rule
that makes counterexamples on tables with an unenforced NULLS NOT DISTINCT
rule INCONCLUSIVE.

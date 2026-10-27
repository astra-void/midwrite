# Phase 12 real-world coverage (Milestone 9)

Pinned revisions, unchanged since Phase 11: saas-starter `6e33e58b`, midday
`51587319` (public, `.corpora/`), school-ledger `4b97f342`,
unnamed-ai-project `39fcfc7c` (private: `git archive`s of the pinned
revisions; only aggregate counts and generic reason categories are
reported). No project was executed. Both binaries were run now, on the same
machine and inputs (`reports/phase12/corpus/`).

## Phase 11 vs Phase 12, stage by stage

| project | phase | tables modeled / refused | generated-key columns | effectful entries | read-only entries | resources (index) | unknown ops | candidates | model-checkable | counterexamples | wall | peak RSS |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| saas-starter | 11 | 5 / 0 | 0 | 8 | 4 | 17 | 274 | 2 | 1 | 1 | 0.06 s | 11 MiB |
| | 12 | 5 / 0 | 0 | 8 | 4 | 17 | 284 | 2 | 1 | 1 | 0.03 s | 11 MiB |
| midday | 11 | 8 / **42** | 0 | 0 | 0 | 0 | 15 382 | 0 | 0 | 0 | 28.07 s | 73 MiB |
| | 12 | **50 / 0** | 48 | 0 | 2 | 0 | 17 603 | 0 | 0 | 0 | 1.13 s | 112 MiB |
| school-ledger | 11 | 9 / **21** | 0 | 0 | 0 | 0 | 9 029 | 0 | 0 | 0 | 3.36 s | 50 MiB |
| | 12 | **30 / 0** | 21 | **21** | 29 | **18** | 12 894 | **9** | 0 | 0 | 0.53 s | 81 MiB |
| unnamed-ai-project | 11 | 7 / **2** | 0 | 1 | 5 | 1 | 1 009 | 0 | 0 | 0 | 0.07 s | 11 MiB |
| | 12 | **9 / 0** | 2 | **3** | 6 | **3** | 1 009 | 0 | 0 | 0 | 0.03 s | 12 MiB |

Entry candidates and extracted entries are unchanged (44/37, 2 864/1 168,
880/546, 74/33): the same entries are analyzed.

## Recognition is not verification: what propagated

```text
schema ─► source effects ─► resource index ─► candidates ─► scenarios ─► model checking
```

* **Schema refusal: 65 → 0.** Every Phase 11 refusal ("primary key column
  … is not modeled") was a generated key: `uuid().defaultRandom()` or
  `$defaultFn` with `crypto.randomUUID()`/a recognized helper. All 65 tables
  are now modeled with `Generated` keys by the same mechanism the
  first-party fixtures use — no project-specific rule exists.
* **midday — recognized, not propagated.** 50 tables, 48 generated-key
  columns, but **0 effectful entries**: midday's query functions receive the
  database as a parameter typed `Database = Awaited<ReturnType<typeof
  connectDb>>`, a `withReplicas(primary, [replica])` database. Binding it
  would treat replica reads as primary reads (invented read-your-writes
  behavior), so its **752 operations** are reported under the new category
  *database handle of an unresolved type* instead. This is the demonstrated
  blocker (Definition of Done 9): read-replica routing is not modeled.
* **school-ledger — propagated to candidates, blocked before model
  checking.** The schema fix plus `getDb()` factories, typed parameters,
  re-exports and repository methods give 21 effectful entries, 18 resources
  and 9 check-then-act candidates. All 9 are **UNSUPPORTED**: each actor
  reaches an unsupported operation before the candidate's operations (7: a
  call whose resolved export is not a function declaration, e.g. a wrapped
  or computed helper; 2: an unsupported expression kind). Their scenarios
  load (a synthesis defect that left a seeded table out of the scenario was
  fixed on the way); none is claimed safe or unsafe. 173 operations sit
  behind `Database | Transaction` parameters of exported helpers (bound when
  inlined with a known handle, not as entries).
* **unnamed-ai-project.** 2 more tables, 1 → 3 effectful entries,
  1 → 3 resources; no candidate.
* **saas-starter.** Unchanged findings. The second `signUp` candidate now
  loads (the local assigned by an unsupported query is recorded on the
  unknown operation instead of being undefined) and is **UNSUPPORTED**: the
  actor reaches `hash` of `bcryptjs` (a package) first. Phase 11 reported it
  as INCONCLUSIVE with a load failure.

**Faithful analysis coverage increased** in the sense that more operations
are bound to correctly modeled tables (school-ledger 0 → 21 effectful
entries; midday's and school-ledger's generated keys are modeled with
event identity rather than refused), but **no new real-world counterexample
exists**: the newly reached code paths stop at operations the model does not
support. No detection threshold was changed for the corpus, and no corpus
result is presented as a defect (no independent review was performed).

## Unknown operations

Unknown counts **rose** (midday 15 382 → 17 603, school-ledger
9 029 → 12 894) because more code is reached: re-exports are followed and
repository methods inlined, which exposes their own unsupported operations.
A falling unknown count is not the goal; their categories are
(`reports/phase12/corpus/SUMMARY.md`):

| category (Phase 12) | saas-starter | midday | school-ledger | unnamed |
| --- | ---: | ---: | ---: | ---: |
| unresolved method call | 133 | 6 053 | 4 603 | 625 |
| call into an unanalyzed package | 55 | 4 335 | 614 | 67 |
| other | 17 | 1 984 | 2 555 | 77 |
| unsupported expression | 42 | 1 679 | 1 706 | 119 |
| database handle of an unresolved type | 0 | 752 | 173 | 0 |
| unresolved identifier | 0 | 717 | 646 | 7 |
| unsupported binding pattern | 4 | 718 | 321 | 16 |
| unresolved call | 4 | 576 | 606 | 0 |
| unsupported transaction shape | 0 | 434 | 127 | 0 |
| inline depth limit | 0 | 179 | 871 | 20 |
| loop over a collection of unknown length | 0 | 167 | 413 | 54 |
| column not modeled (schema) | 12 | 6 | 157 | 9 |
| table not modeled (schema) | 0 | 0 | 22 | 13 |
| unsupported query (raw SQL) | 0 | 0 | 60 | 0 |
| unsupported query shape | 17 | 3 | 20 | 2 |

"table not modeled" in school-ledger are Drizzle `alias()` self-join
handles and local variables holding a table; "column not modeled" are
timestamp/JSON columns.

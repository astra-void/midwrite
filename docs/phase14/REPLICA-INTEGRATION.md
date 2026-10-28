# Phase 14 Track A: replica scenario integration

## A1. What the Phase 13 documents said, and what the code does

Phase 13 left two contradicting statements: the analyzer page said the
scenario loader built its extraction request with `replicas: false` and no
replica factories (so midday's `toggleCustomerPortal` was UNSUPPORTED for
that reason), while the model page said synthesized scenarios carry the
factories and the `replicas` feature. The merged Phase 13 code does the
latter:

| step | where | what is preserved |
| --- | --- | --- |
| discovery request | `discover/src/extract.rs` `extract_entry` | `replicas` (the project builds a replicated database), declared `replicaFactories` |
| synthesized scenario | `discover/src/synth.rs` `Plan::to_json` | `extraction.replicaFactories` (verbatim), `features: ["replicas"]` when an actor reads through a replica, `foreign-keys` when the tables carry foreign keys |
| loader request | `model/src/scenario.rs` | `replicas: features.replicas`, `replica_factories: extraction.replica_factories` |

The analyzer page's statement was written in the analyzer worktree before
the merge; it was corrected at the Phase 13 closeout
(`docs/phase13/REPLICA-AWARE-ANALYSIS.md` §4, `MIDDAY-COVERAGE.md` closeout
note). Nothing had to change in the propagation itself.

## A2. Round-trip validation (new)

Nothing *checked* that the loader re-extracts what discovery extracted; a
silent difference (a replica read re-extracted as a primary read, a handle
left unbound) would have model-checked another program. Phase 14 adds the
check before any model checking:

```text
original project ─► discovery extraction (codes per entry)
                 ─► synthesized scenario ─► scenario loader ─► re-extracted actor codes
                                                       │
                          compare the normalized effect traces ◄┘
```

`discover/src/synth.rs` `effect_trace` normalizes one extraction: per code
vector, in code order, every database statement as
`source connection kind table` (`tx`/`pool`/`replica`; `select` with its
lock strength, `count`, `insert`/`upsert`, `update`, `delete`), every
`BEGIN`/`COMMIT`/`ROLLBACK`, external call and unknown operation, each with
its source location. Local computation, jumps, and the two steps only the
loader adds (`Op::Respond`, `Op::ReturnValue`) are not effects. The first
solo load of each actor in synthesis compares the trace with discovery's
(`round_trip`); a difference makes the scenario UNSUPPORTED with "scenario
round-trip: the loader's extraction of actor `…` differs from discovery's:
code *i*, effect *j*: discovery has `…`, the loader has `…`".

Test: `synth::tests::round_trip_detects_a_replica_read_reextracted_as_a_primary_read`
(a replica read re-extracted as a pool read is detected; the loader's extra
response step is not a difference). On every first-party fixture and the
corpus the check passes (no candidate became UNSUPPORTED through it; the
full test suite exercises it on every synthesized scenario).

## A3. Midday checkpoint

Same input as Phase 13 (midday `51587319`, `corpora/contracts/midday.json`).
With the Phase 14 binary: see `REAL-WORLD-RESULTS.md` for the measured
table. The replica-reading `toggleCustomerPortal` passes the round trip,
loads with the `replicas` feature and the declared factory, and reaches
its solo run; its first blocker is the call `nanoid(21)` on its path.

The two write-skew candidates (`leaveTeam`, `deleteTeamMember`) stay
BLOCKED: no invariant is declared, and CommitRift does not infer a
multi-row invariant from the existence of a candidate.

## A4. Replica semantics

Unchanged: one asynchronous replica, unbounded lag, reads on a replayed
prefix of the commit sequence (`docs/phase13/REPLICA-CONSISTENCY.md`). A
replica read is never treated as a primary read. No PostgreSQL streaming
replication was run in Phase 14 either: replica results remain
**model-only**, and the original-code runner refuses schedules that depend
on replication.

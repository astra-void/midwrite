# Read replicas in the model (Track C, model side)

Handle provenance and routing in the analyzer: `REPLICA-AWARE-ANALYSIS.md`.
Midday numbers: `MIDDAY-COVERAGE.md`.

## 1. Assumed replication model (stated with every replica scenario)

* **Asynchronous streaming replication** to one hot standby: the replica
  replays the primary's commits **in commit order**, so what it shows is a
  prefix of the primary's commit sequence.
* **Unbounded lag**: the replica may stay behind for the whole schedule. No
  numerical lag distribution, no timing guarantee, no synchronous
  replication (`synchronous_commit = remote_apply` would make the replica
  current at commit; no configuration evidence for it was found in the
  corpus, and it is not modeled).
* One replica. Drizzle's `withReplicas` picks a replica per read; with
  several replicas each would have its own prefix — not modeled.
* Reads on the replica are plain reads: a hot standby refuses writes and row
  locks, so such statements are refused by the loader (and the extractor
  makes them unknown).

## 2. Representation

| | |
| --- | --- |
| state | `State.replica_seq`: the length of the replayed prefix; initial data is visible |
| transition | `TKind::Replicate`: replay the next commit (`replica_seq += 1`), enabled while `replica_seq < commit_seq`; an optional environment choice (not required for quiescence) |
| statement | `Conn::Replica` (`select`, `count`, joins): runs outside any primary transaction on `Snapshot { seq: replica_seq }`; it takes no locks and never waits |
| feature | scenario `features: ["replicas"]`; without it a replica statement is refused ("needs the feature `replicas`") — a replica read is never bound to the primary |
| DSL | `let p = replica select profiles where @id == 1` |
| oracle | the lockstep PostgreSQL oracle refuses replica models (one server) |
| original code | a schedule that depends on replication is refused for original-code verification ("replication is not steered against original code") |

**Canonical encoding.** The replica is one more snapshot holder (each
committed version records whether the replica sees it), and every committed
version carries its distance from the replica's prefix: two states whose
unreplicated commits would be replayed in a different order are different
states.

**Footprints.** A replica read records `Snapshot`; `Replicate` records
`CommitWrite`: they conflict with each other, and the replica read is
(conservatively) dependent on primary commits. On a two-writer / one-reader
workload the independence audit is clean and the sleep-set search, classic
DPOR and Source-DPOR agree with the unreduced search; the reader can observe
the initial row, the first write and the second, and never the second
without the first (`model/tests/phase13.rs`).

## 3. Read-your-writes (Vertical slice 4, model only)

`fixtures/phase13/model/replica-read-your-writes.json`: update a profile
through the primary, read it back through the replica, return the name;
declared invariant "the caller sees its write" (`returned`).

* replica read → **counterexample** (2 transitions: the read happens before
  the replica replays the commit; returns `"old"`);
* the same read on the primary (`db.$primary`) → no counterexample;
* once the replica has replayed the commit, the read returns `"new"`.

This is a **model-only** result. No PostgreSQL streaming replication was
run: the claim is about the stated replication model, not about a measured
replica. A stale read is reported only against a declared property; it is
not by itself a defect.

## 4. Discovery

A synthesized scenario gets the `replicas` feature when one of its actors
reads through a replica, and carries the project's declared
`replicaFactories`, so that the loader extracts exactly what discovery
extracted. On the frozen evaluation set, the replica-backed check-then-act
`addLabel` (check on the replica, insert on the primary) is a model
counterexample — a true positive the Phase 12 binary could not reach.

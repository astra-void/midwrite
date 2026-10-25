# PostgreSQL validation in Phase 7

## Versions

| Platform | PostgreSQL | Isolation backend | Result |
| --- | --- | --- | --- |
| macOS 27.0.1 arm64 (host) | 17.11 (Homebrew) | sandbox-exec | full suite 412 passed (see final gates) |
| Debian 12 container, Linux 7.0.12 aarch64 (`scripts/linux-replay-docker.sh`, existing Phase 6 image) | 15.19 | bubblewrap | 411 passed, 0 failed, 0 ignored, replay required (`reports/phase7/linux/`) |

PostgreSQL 15 is not installed on the macOS host (only `postgresql@17`);
version 15 was exercised only in the Linux container. No x86-64 run.
The Linux run excludes `.corpora/` (external sources), so the original
saas-starter extraction test skips there.

## New differential workloads (model vs model SQL on PostgreSQL)

`crates/commitrift-model/tests/phase7.rs`, every schedule of each
configuration, each under READ COMMITTED, REPEATABLE READ and
SERIALIZABLE:

| Workload | Constructs |
| --- | --- |
| left-join-membership | LEFT JOIN read vs concurrent insert into the joined table; generated ids; `RETURNING` |
| projected-join | projection of a nested table and a joined column |
| sequence-rollback | sequence allocation by a rolled-back insert; constant default; `RETURNING` |
| unique-generated-on-conflict | UNIQUE(teamId, email), generated ids, `ON CONFLICT DO NOTHING … RETURNING`, a third transaction rolling back after `RETURNING` |
| inner-join-vs-delete | two INNER JOIN reads in one transaction (READ COMMITTED snapshots vs REPEATABLE READ) racing a delete of the joined row |

**102 schedules, 15 configurations, 0 disagreements** on 17.11 (macOS) and
on 15.19 (Linux). Partial-order reduction keeps every reachable state on
these workloads (full vs reduced exploration compared).

The Phase 6 matrix (1 695 schedules, 60 configurations: lost update, write
skew, check-then-act, deadlocks, `ON CONFLICT`, row locking, `SKIP
LOCKED`/`NOWAIT`, CHECK, delete vs update, serialization failures,
READ COMMITTED statement snapshots) still passes on both versions, as part
of the suites above.

## Observed flakiness

Deadlock-victim steering remains timing-based (Phase 6). While the Phase 6
baseline tree was re-tested in a second checkout with the machine under
load (a parallel compilation), `deadlock_workloads` disagreed once:
PostgreSQL chose the other member of the wait-for cycle as victim. Run
alone afterwards it passed 3 of 3 times. This is reported as a
disagreement by design, never forced.

## SERIALIZABLE

Unchanged conservative serialization-graph abstraction. Joins record
whole-table reads under SERIALIZABLE, which can make the model abort more
than PostgreSQL with index scans; on the tested workloads no disagreement
appeared. No claim of SSI equivalence; the Phase 6 one-way plan-dependent
difference (sequential scans) still applies.

## Three comparisons

This document covers *abstract model vs PostgreSQL*. *Drizzle SQL vs model*
and *original application vs model* are in
[`DIFFERENTIAL-VERIFICATION.md`](DIFFERENTIAL-VERIFICATION.md); the
original-code tests ran on both PostgreSQL versions.

---
title: "Phase 10 correction: postgres-js shutdown through the fault proxy"
author: Astra-void
date: 2026-10-27
branch: phase-8
cost: { minutes: 24, tokens: 39862 }
touches:
  - Cargo.lock
  - crates/commitrift-analyzer/tests/fixtures.rs
  - crates/commitrift-cli/src/model_check.rs
  - crates/commitrift-cli/tests/phase9.rs
  - crates/commitrift-model/Cargo.toml
  - crates/commitrift-model/src/engine.rs
  - crates/commitrift-model/src/explore.rs
  - crates/commitrift-model/src/footprint.rs
  - crates/commitrift-model/src/html.rs
  - crates/commitrift-model/src/lib.rs
---

## Decided
- The postgres-js harness ends its client with sql.end({ timeout: 5 }).
## Tried and ruled out
- Attributing the slow postgres-js shutdown to the severed connection (an earlier Phase 10 learning said so): the restarted worker of a crash scenario, whose connections had no fault, also needed the 5 s timeout. The Phase 8 postgres-js harness (no proxy) ends normally.
## Watch out
- postgres-js 3.4.5 sql.end() without a timeout does not return when its connections go through the CommitRift fault proxy, fault or not; the cause is not established. Every postgres-js proxied run costs ~5 s extra (median 6.4 s vs 1.3 s for node-postgres).
- The replay uses PostgreSQL 17.11 binaries on this Mac although pg_config on PATH says 18.6: read postgresVersion from a report instead of trusting PATH.

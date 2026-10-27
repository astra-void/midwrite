---
title: "Phase 10: source-set DPOR, adversarial POR testing, live connection loss"
author: Astra-void
date: 2026-10-26
branch: phase-8
cost: { minutes: 30, tokens: 924808 }
touches:
  - Cargo.lock
  - crates/commitrift-cli/src/model_check.rs
  - crates/commitrift-cli/tests/phase9.rs
  - crates/commitrift-model/Cargo.toml
  - crates/commitrift-model/src/engine.rs
  - crates/commitrift-model/src/explore.rs
  - crates/commitrift-model/src/footprint.rs
  - crates/commitrift-model/src/html.rs
  - crates/commitrift-model/src/lib.rs
  - crates/commitrift-model/src/model.rs
---

## Decided
- Classic DPOR's 1000x blowup is duplicate trace exploration (984,511 runs = 148 Mazurkiewicz traces at team-4) plus no state cache; fixed by source-set DPOR (explore/sdpor.rs) with sleep sets and a state cache whose entries carry an interned summary of (transition, footprint) events below them.
- The event that enabled a transition is DEPENDENT with it (a race candidate), never an ordered predecessor: Crash is enabled by the first step of ANY task (disjunctive), so "enabler = necessary cause" loses terminals.
- Handle EVERY race, not just the last: conditional enabling (Crash needs a started task) makes the earlier race the one that matters. The Phase 9 classic DPOR had this bug (adversarial seed 165, 9 transitions); fixed in explore_dpor too.
- A sleeping member of a choice class (Step / LostCommitted / LostNotCommitted of one op) makes its awake siblings backtrack obligations.
- Terminal states are compared modulo cross-channel external-effect order (State::canonical_observable); the exact canonical key stays the state identity.
- Live connection loss = proxy sever_client + PlanStep::ConnectionLoss; the worker is never signalled; a dead worker is DIVERGED with stderr quoted.
- Sleep sets stay the default reduction.
## Tried and ruled out
- Enabling edges as ordered HB edges (unsound for disjunctive enabling).
- Trusting random seeds without a mutation table: tests/por_mutation.rs shows which deleted access kinds the corpus can detect.
## Watch out
- node-postgres emits 'error' on a checked-out client whose socket ends; without client.on('error') the Node process dies (Unhandled 'error' event). Harnesses need the listener; ledger-live/verify/harness-unhandled.ts documents the crash.
- postgres-js reads pg_type on every new connection: proxy statement faults must target writes only (proxy::is_write_sql).
- postgres-js sql.end() did not return after a severed connection; harness uses end({ timeout: 5 }).
- With a live worker, wait for the held operation's own backend pid to end, not for all sessions of the actor.
- Do not rm -rf /private/tmp/cr-* (shared temp); use COMMITRIFT_KEEP_ARTIFACTS=1 and read the newest dir.

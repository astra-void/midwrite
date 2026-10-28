# Phase 14 performance

Same machine (Darwin 27.0.0 arm64), Phase 13 final binary vs Phase 14
binary, same inputs. Wall times are indicative only: the corpus numbers are
medians of 3 runs (`reports/phase14/corpus/SUMMARY.md`); the fixture
numbers below are single runs taken while the final test suite was running
on the same machine.

## Discovery on the corpus (`--model-check`, median of 3, peak RSS)

| project | Phase 13 | Phase 14 |
| --- | --- | --- |
| saas-starter | 0.03 s / 12 MiB | 0.04 s / 13 MiB |
| midday, no declaration | 1.07 s / 119 MiB | 1.21 s / 143 MiB |
| midday, declared factory | 1.12 s / 121 MiB | 1.24 s / 148 MiB |
| school-ledger | 0.56 s / 98 MiB | 0.70 s / 122 MiB |
| unnamed-ai-project | 0.03 s / 13 MiB | 0.04 s / 15 MiB |

The cost is more extracted code (midday effectful entries 47 → 81,
school-ledger 21 → 70) and the JavaScript value layer; memory grew by at
most 27 MiB. The discovery cache key now includes the value semantics
(`CACHE_VERSION` `commitrift-discover-cache-4`), so caches of earlier
builds are not reused.

## State spaces

| input | Phase 13 | Phase 14 |
| --- | --- | --- |
| `fixtures/phase12/workspace-app` (19 candidates, 28 scenarios checked) | 635 states | **635 states** (the `js-values` feature changes no state count there) |
| `fixtures/phase13/eval-app` | 13 candidates, 14 scenarios, 311 states, 0.04 s | 20 candidates, 30 scenarios, 1 015 states, 0.06 s |
| `fixtures/phase14/eval-app` | 12 candidates, 20 scenarios, 362 states | 20 candidates, 28 scenarios, 520 states |

States grow with what is newly checked (more candidates and scenarios),
not per scenario. Earlier scenarios keep their state bytes: with the
features off, the reachable-state digests of the 83 loadable Phase 6–13
fixture scenarios are identical to the pre-Phase-14 build
(`VALUE-SEMANTICS.md`), and 17 scenarios keep byte-identical reports and
canonical state (`DRIZZLE-DIFFERENTIAL.md` D7). New values only appear in
the canonical encoding when a state holds them (tags 6–9).

## Partial-order reduction

Unchanged algorithms. New footprints: multi-row inserts (per row, as the
single-row insert), clock readings (actor-local), generated values from
package calls and the new local values (actor-local, no footprint). Agreement
of sleep sets, classic DPOR and Source-DPOR with the unreduced search on 86
Drizzle models (344 runs) and 9 value-domain models × 3 levels.

# Performance

Raw: `reports/phase11/evidence/SUMMARY.md` (release binary sha256
`6141d12d…`, macOS arm64; medians of 3 runs, peak RSS the maximum, from
`/usr/bin/time -l`).

## Static analysis is unchanged

`commitrift check` does not run discovery. Its outputs are byte-identical to
the Phase 10 binary on all 144 corpus cases (`reports/phase11/final/corpus-identity.txt`);
its cost is the same within noise:

| | Phase 10 binary | Phase 11 binary |
| --- | ---: | ---: |
| commerce-app `check` | 0.01 s, 7.3 MiB | 0.01 s, 7.3 MiB |
| midday `check` | 0.21 s, 91.7 MiB | 0.22 s, 94.9 MiB |

## Cost centers of discovery

`commerce-app` (19 modules, 50 entry candidates), one run with timings:

| phase | ms |
| --- | ---: |
| index (entries, schema, probe load) | 5 |
| extraction (42 entries) | 22 |
| candidate detection | 0 |
| synthesis (31 scenarios, engine runs, serial outcomes) | 9 |
| model checking (31 scenarios, 781 states in total, sleep sets) | 8 |
| total | 77 |

| | wall | peak RSS |
| --- | ---: | ---: |
| discovery only | 0.06 s | 10.1 MiB |
| discovery + model checking | 0.08 s | 10.5 MiB |
| discovery + model checking, source-DPOR | 0.08 s | 10.5 MiB |

The synthesized scenarios are small (5–192 states): bounded two-actor
models over a few rows. The algorithm used is recorded per scenario;
sleep sets stay the default.

## Scaling: midday

2 022 modules, 2 864 entry candidates: 1 696 are not extracted (no
database receiver or declared external in their import closure), 1 168 are.

| | wall | peak RSS |
| --- | ---: | ---: |
| discovery only, no cache | 30.0 s | 73.1 MiB |
| discovery only, cold cache (writes it) | 36.6 s | — |
| discovery only, warm cache (1 168 hits) | 7.7 s | — |

Extraction dominates (≈ 98 %): each extraction re-runs file discovery and
builds an import resolver over the whole project (≈ 25 ms per entry on
midday), as the scenario loader does for one scenario. Sharing the file list
and resolver across entries is the obvious next step; it was not done
because extraction is the loader's code path and the identity of
model-check results depends on it. The warm-cache report equals the cold
one apart from timings and cache counters.

## Candidate explosion

Pairing goes through the `(table, column)` resource index, never all pairs
of functions; helpers other entries inline are not partners; each
unordered pair is formed once; at most `--max-partners` (6) partners per
candidate, the rest counted (`partnersTruncated`). Scenario work is ordered
by static strength and capped by `--max-candidates` (64), per-scenario
`--max-states` (200 000) and `--max-wall-ms` (60 s), and a total
`--max-total-states` (5 000 000); skipped work is counted, never dropped
silently. No corpus run reached a budget.

## Original-code verification

Each runtime run of a generated scenario takes 1.4–6 s (process start,
disposable PostgreSQL, coordinated statements); `--source-repeat 3` for 10
scenarios took ≈ 2.5 minutes. It is never part of `discover`.

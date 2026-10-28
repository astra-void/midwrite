# Phase 15 performance

Raw: `reports/phase15/performance/` (`scripts/phase15-perf.py`, median of
3 runs, peak RSS the maximum, macOS arm64, release build) and
`reports/phase15/corpus/`.

## World search off (`--max-worlds 1`) versus default (8)

| application | max worlds | candidates | counterexamples (in non-base worlds) | worlds built | states | synthesis ms | model checking ms | ms per candidate | wall s | peak RSS MiB |
| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Phase 13 set | 1 | 20 | 9 | 30 | 1015 | 7 | 6 | 2.8 | 0.068 | 12.2 |
| Phase 13 set | 8 | 20 | 10 (1) | 32 | 1056 | 16 | 10 | 4.0 | 0.082 | 12.3 |
| Phase 14 set | 1 | 20 | 7 | 28 | 520 | 6 | 1 | 2.5 | 0.061 | 12.3 |
| Phase 14 set | 8 | 20 | 8 (1) | 30 | 558 | 10 | 1 | 3.0 | 0.073 | 12.5 |
| Phase 15 set | 1 | 19 | 4 | 20 | 685 | 8 | 5 | 2.6 | 0.059 | 12.1 |
| Phase 15 set | 8 | 19 | 8 (4) | 28 | 803 | 12 | 6 | 3.6 | 0.075 | 12.2 |
| worlds-app | 1 | 3 | 0 | 6 | 87 | 0 | 0 | 2.7 | 0.018 | 9.9 |
| worlds-app | 8 | 3 | 2 (2) | 8 | 105 | 0 | 1 | 4.0 | 0.021 | 10.0 |
| commerce-app (Phase 11) | 1 | 22 | 12 | 31 | 800 | 10 | 11 | 3.3 | 0.076 | 12.2 |
| commerce-app (Phase 11) | 8 | 22 | 12 (0) | 32 | 836 | 7 | 10 | 3.0 | 0.075 | 12.2 |

* Overhead of the search: +0–30 % wall time, no measurable memory change.
  Most candidates stop early (a counterexample in the base world, or few
  derived classes); the largest growth is on the Phase 15 set (+8 worlds,
  +17 % states).
* Time to first counterexample is bounded by the world order: in every
  non-base case above the counterexample came from the first or second
  derived world (the `worlds` list of each candidate's coverage section).
* State-space growth per world is unchanged (worlds are small: 5–40
  states each in these sets).

## Corpus

| project | Phase 14 binary | Phase 15 binary |
| --- | --- | --- |
| midday, declared factory (2 022 modules, 1 168 entries) | 1.36 s, 144 MiB | 1.32 s, 148 MiB |
| school-ledger | 0.71 s, 120 MiB | 0.76 s, 121 MiB |
| saas-starter | 0.04 s, 13 MiB | 0.04 s, 13 MiB |

No candidate of these projects reaches a checkable world, so the search
adds only the synthesis attempts of the few domain worlds.

## Combinatorial bounds

Worlds change one relationship at a time (no Cartesian product); at most
`--max-worlds` attempts per actual variant; duplicates are detected by
fingerprint before model checking; the global `--max-total-states` still
caps all model checking. Where completeness would need more (every value
of a domain, every combination of relationships, more actors), the
coverage section lists it as unexplored instead of paying for it.

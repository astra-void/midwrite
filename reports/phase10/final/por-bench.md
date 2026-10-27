# Phase 10 reduction benchmark

Darwin arm64, release build, `--no-minimize`, wall limit 120000 ms, median of 3 for runs under 10 s (one run otherwise). Wall = the report's exploration time; RSS = peak resident set of the process.


## 1 completely independent actors (5)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 3125 | 12500 | 58 | — | 6.0 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 3125 | 3124 | 31 | — | 6.0 |  |
| classic DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 21 | 20 | 0 | — | 4.9 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 21 | 20 | 0 | — | 5.1 | 0 / 0 / 0 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 21 | 20 | 0 | — | 4.9 | 0 / 0 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 21 | 20 | 0 | — | 5.1 | 0 / 0 / 0 |

## 2 mostly independent actors (4, one shared counter)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 1296 | 4320 | 20 | — | 5.6 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1296 | 1416 | 13 | — | 5.5 |  |
| classic DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 14394506 | 14394505 | 94646 | — | 5.0 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 436 | 1039 | 14 | — | 5.5 | 3131 / 604 / 8174 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 161 | 160 | 1 | — | 5.2 | 116 / 23 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 88 | 106 | 1 | — | 5.4 | 114 / 20 / 57 |

## 3 highly conflicting transactions (4, lost update)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | COUNTEREXAMPLE_FOUND — reduction refused: disabled in the scenario options | 22 | 22 | 0 | 0 | 5.0 |  |
| sleep sets | COUNTEREXAMPLE_FOUND | 22 | 21 | 0 | 0 | 5.0 |  |
| classic DPOR | COUNTEREXAMPLE_FOUND | 26 | 25 | 0 | 0 | 5.0 |  |
| sdpor stateful | COUNTEREXAMPLE_FOUND | 22 | 22 | 0 | 0 | 5.1 | 9 / 6 / 3 |
| sdpor stateless+sleep | COUNTEREXAMPLE_FOUND | 22 | 21 | 0 | 0 | 5.0 | 6 / 5 / 0 |
| sdpor stateful+sleep | COUNTEREXAMPLE_FOUND | 22 | 21 | 0 | 0 | 5.1 | 6 / 5 / 0 |

## 3b highly conflicting, safe (4 atomic increments of one row)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 1888 | 6232 | 31 | — | 5.6 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1888 | 4638 | 26 | — | 5.6 |  |
| classic DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 17667615 | 17667614 | 81117 | — | 4.9 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1730 | 5220 | 61 | — | 6.2 | 30796 / 2256 / 35192 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 535328 | 535327 | 2790 | — | 5.1 | 310920 / 87419 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1804 | 4843 | 55 | — | 6.2 | 18001 / 1857 / 21915 |

## 4 shared pool contention (4 tasks, pool of 2)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 140 | 226 | 0 | — | 4.8 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 140 | 204 | 1 | — | 4.8 |  |
| classic DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 3945 | 3944 | 41 | — | 4.8 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 140 | 222 | 3 | — | 5.1 | 430 / 90 / 404 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1217 | 1216 | 13 | — | 4.8 | 836 / 279 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 140 | 210 | 3 | — | 5.0 | 349 / 68 / 256 |

## 5 read/write skew (3 doctors, SERIALIZABLE)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 328 | 522 | 3 | — | 5.1 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 328 | 423 | 3 | — | 5.2 |  |
| classic DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 25587 | 25586 | 157 | — | 5.0 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 270 | 388 | 4 | — | 5.3 | 720 / 160 / 673 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 3708 | 3707 | 24 | — | 5.0 | 3074 / 1247 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 270 | 357 | 4 | — | 5.3 | 492 / 129 / 388 |

## 6 in-flight COMMIT + reconciliation (Phase 9)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | COUNTEREXAMPLE_FOUND — reduction refused: disabled in the scenario options | 11 | 10 | 0 | 0 | 4.8 |  |
| sleep sets | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 4.8 |  |
| classic DPOR | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 4.8 |  |
| sdpor stateful | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 4.9 | 4 / 8 / 0 |
| sdpor stateless+sleep | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 4.8 | 4 / 8 / 0 |
| sdpor stateful+sleep | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 5.0 | 4 / 8 / 0 |

## 6b in-flight COMMIT, lookup + unique key, concurrent delivery (Phase 9)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 305 | 612 | 2 | — | 5.1 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 305 | 551 | 2 | — | 5.0 |  |
| classic DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 17009 | 17008 | 62 | — | 4.9 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 291 | 520 | 3 | — | 5.3 | 1257 / 187 / 1230 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 3215 | 3214 | 13 | — | 5.0 | 2398 / 681 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 295 | 505 | 4 | — | 5.4 | 833 / 166 / 685 |

## 7 crash/retry (4 actors)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 4625 | 32600 | 142 | — | 6.6 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 4625 | 24024 | 120 | — | 6.5 |  |
| classic DPOR | INCONCLUSIVE (TRUNCATED at the wall limit) | 27442960 | 27442959 | 120000 | — | 5.1 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 3249 | 11960 | 126 | — | 6.7 | 101556 / 6476 / 78368 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 405625 | 405624 | 2693 | — | 5.4 | 592500 / 40507 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 3249 | 10584 | 107 | — | 6.6 | 61236 / 5107 / 49440 |

## 8 multiple tasks per process (3 tasks + 1 actor)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 20 | 36 | 0 | — | 4.7 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 20 | 28 | 0 | — | 4.7 |  |
| classic DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 52 | 51 | 0 | — | 4.8 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 17 | 29 | 0 | — | 4.8 | 46 / 14 / 18 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 38 | 37 | 0 | — | 4.8 | 40 / 11 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 17 | 25 | 0 | — | 4.8 | 36 / 10 / 12 |

## 9 queue redelivery (Phase 8 job)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | COUNTEREXAMPLE_FOUND — reduction refused: disabled in the scenario options | 11 | 10 | 0 | 0 | 8.3 |  |
| sleep sets | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 8.3 |  |
| classic DPOR | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 8.3 |  |
| sdpor stateful | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 8.3 | 6 / 2 / 0 |
| sdpor stateless+sleep | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 8.4 | 6 / 2 / 0 |
| sdpor stateful+sleep | COUNTEREXAMPLE_FOUND | 11 | 10 | 0 | 0 | 8.3 | 6 / 2 / 0 |

## 10a team-app, 4 actors (extracted)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 1984 | 5344 | 64 | — | 9.3 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 1984 | 2291 | 44 | — | 9.4 |  |
| classic DPOR | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 3476615 | 3476614 | 37973 | — | 8.7 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 930 | 1665 | 35 | — | 9.0 | 5969 / 767 / 6669 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 697 | 696 | 10 | — | 9.0 | 650 / 155 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 454 | 574 | 12 | — | 9.2 | 655 / 156 / 118 |

## 10b team-app, 5 actors (extracted)

| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| unreduced | NO_COUNTEREXAMPLE_WITHIN_BOUNDS — reduction refused: disabled in the scenario options | 13984 | 46160 | 599 | — | 11.8 |  |
| sleep sets | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 13984 | 18042 | 398 | — | 12.1 |  |
| classic DPOR | INCONCLUSIVE (TRUNCATED at the wall limit) | 10290929 | 10290928 | 120000 | — | 9.0 |  |
| sdpor stateful | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 6168 | 13076 | 322 | — | 11.1 | 68074 / 6988 / 66236 |
| sdpor stateless+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 6040 | 6039 | 92 | — | 9.5 | 6354 / 1461 / 0 |
| sdpor stateful+sleep | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | 2562 | 3849 | 93 | — | 10.3 | 6317 / 1355 / 2039 |

## Actor scaling (team-app), memory

| actors | mode | states | transitions | wall ms | peak RSS MiB | key table + interner + edges + nodes bytes | summaries bytes (estimate) |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 3 | sleep sets | 280 | 294 | 4 | 8.8 | 50697 | — |
| 3 | sdpor stateful+sleep | 96 | 104 | 2 | 8.7 | 21513 | 13384 |
| 3 | sdpor stateless+sleep | 108 | 107 | 1 | 8.6 | 0 | 0 |
| 4 | sleep sets | 1984 | 2291 | 44 | 9.4 | 272453 | — |
| 4 | sdpor stateful+sleep | 454 | 574 | 12 | 9.3 | 57925 | 36360 |
| 4 | sdpor stateless+sleep | 697 | 696 | 10 | 9.0 | 0 | 0 |
| 5 | sleep sets | 13984 | 18042 | 411 | 11.8 | 1680001 | — |
| 5 | sdpor stateful+sleep | 2562 | 3849 | 92 | 10.4 | 377473 | 127312 |
| 5 | sdpor stateless+sleep | 6040 | 6039 | 91 | 9.5 | 0 | 0 |
| 6 | sleep sets | 97408 | 140975 | 3613 | 28.3 | 14034161 | — |
| 6 | sdpor stateful+sleep | 15358 | 27345 | 755 | 13.4 | 1762545 | 516344 |
| 6 | sdpor stateless+sleep | 64513 | 64512 | 1021 | 10.1 | 0 | 0 |
| 7 | sleep sets | 669568 | 1084010 | 30811 | 117.9 | 113256237 | — |
| 7 | sdpor stateful+sleep | 91407 | 190830 | 6046 | 32.7 | 14165805 | 2177648 |
| 7 | sdpor stateless+sleep | 807496 | 807495 | 13482 | 10.6 | 0 | 0 |

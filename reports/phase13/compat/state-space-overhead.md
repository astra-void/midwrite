# State-space overhead of the Phase 13 semantics (same scenarios, feature on vs off)

Discovered workspace-app scenarios (`foreign-keys` emitted by discovery vs removed):

| scenario | with foreign keys | without | states ratio |
| --- | --- | --- | ---: |
| cr-synth-0ee307-actual.json | COUNTEREXAMPLE_FOUND, 15 states, 14 transitions | COUNTEREXAMPLE_FOUND, 15 states, 14 transitions | 1.00 |
| cr-synth-0ee307-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 26 states, 27 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 26 states, 27 transitions | 1.00 |
| cr-synth-0fca8f-actual.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 23 states, 25 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 23 states, 25 transitions | 1.00 |
| cr-synth-0fca8f-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 23 states, 25 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 23 states, 25 transitions | 1.00 |
| cr-synth-2839f3-actual.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 10 states, 9 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 10 states, 9 transitions | 1.00 |
| cr-synth-34933d-actual.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | 1.00 |
| cr-synth-34933d-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | 1.00 |
| cr-synth-4796f3-actual.json | COUNTEREXAMPLE_FOUND, 15 states, 14 transitions | COUNTEREXAMPLE_FOUND, 15 states, 14 transitions | 1.00 |
| cr-synth-4796f3-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 26 states, 27 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 26 states, 27 transitions | 1.00 |
| cr-synth-6c9b41-actual.json | COUNTEREXAMPLE_FOUND, 25 states, 26 transitions | COUNTEREXAMPLE_FOUND, 21 states, 20 transitions | 1.19 |
| cr-synth-6c9b41-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 37 states, 38 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 37 states, 38 transitions | 1.00 |
| cr-synth-6d52c6-actual.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 23 states, 25 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 23 states, 25 transitions | 1.00 |
| cr-synth-6d52c6-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 23 states, 25 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 23 states, 25 transitions | 1.00 |
| cr-synth-8fb7cc-actual.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | 1.00 |
| cr-synth-8fb7cc-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | 1.00 |
| cr-synth-95906b-actual.json | COUNTEREXAMPLE_FOUND, 25 states, 26 transitions | COUNTEREXAMPLE_FOUND, 21 states, 20 transitions | 1.19 |
| cr-synth-95906b-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 37 states, 38 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 37 states, 38 transitions | 1.00 |
| cr-synth-aab8d0-actual.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | 1.00 |
| cr-synth-aab8d0-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | 1.00 |
| cr-synth-b7e58c-actual.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | 1.00 |
| cr-synth-b7e58c-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 16 states, 17 transitions | 1.00 |
| cr-synth-c2de56-actual.json | COUNTEREXAMPLE_FOUND, 9 states, 8 transitions | COUNTEREXAMPLE_FOUND, 9 states, 8 transitions | 1.00 |
| cr-synth-c57a1a-actual.json | COUNTEREXAMPLE_FOUND, 15 states, 14 transitions | COUNTEREXAMPLE_FOUND, 15 states, 14 transitions | 1.00 |
| cr-synth-c57a1a-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 26 states, 27 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 26 states, 27 transitions | 1.00 |
| cr-synth-f12f55-actual.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 40 states, 47 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 40 states, 43 transitions | 1.00 |
| cr-synth-f12f55-distinct-key.json | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 53 states, 55 transitions | NO_COUNTEREXAMPLE_WITHIN_BOUNDS, 53 states, 55 transitions | 1.00 |
| **total** | 579 states | 571 states | 1.01 |

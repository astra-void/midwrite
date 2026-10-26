# State-space storage

How the explicit-state search stores the states it has visited, why that
needs much less memory than in Phase 8, and why it is still exact.

## Phase 8

Each visited state was stored as its flat canonical encoding, in a hash map
from encoding to state id. `visitedBytes` was `key length + 96` per state. At
6 actors on the benchmark workload that was 4 758 bytes per state and 463 MB in
total, with a peak RSS of 735 MiB (`BASELINE.md`). About 98 % of the bytes
were the encoding itself: every row carried its column names as strings,
statements were rendered with `format!("{:?}")`, and every locals map
repeated its keys. Memory therefore grew with the *size* of a state, not
only with the number of states.

## Phase 9

1. **Fragmented canonical encoding** (`state.rs`, `Enc::cut`). The encoder
   still writes the same canonical bytes, but it marks cut points between
   components: database tables, each thread, transactions, locks, orphans and
   pools. Each component's bytes are one *fragment*.
2. **Interner** (`state::Interner`). Each distinct fragment is stored once and
   gets a `u32` id, so a state becomes a sequence of ids. Most components
   repeat across states (an actor's local state at a given program counter,
   an unchanged table), so a 4.7 KB state shrinks to a few dozen ids. The
   interner keeps the fragment bytes and compares them exactly, never by hash
   alone.
3. **Key table** (`keytable.rs`). The id sequences are stored back to back in
   one byte arena as LEB128 varints: one byte per id while there are fewer
   than 128 distinct fragments, two bytes below 16 384. The open-addressing
   table holds 4-byte references into the arena. A lookup hashes the encoded
   key to pick a slot, then **compares the keys byte for byte**. The varint
   encoding is injective, so equal byte strings mean equal id sequences,
   which in turn mean equal canonical encodings.
4. **Edges** (`explore.rs`, `Edges`). Child lists form one linked list in two
   flat vectors, instead of a `Vec` per node.

### Why it is exact

Two states are merged only if their canonical encodings are equal: every
fragment matches byte for byte in the interner, and the id sequences match
byte for byte in the key table. A hash collision can cost a probe; it can
never merge two different states. No step relies on hash equality.
Tests (`keytable.rs`):

* `colliding_hashes_do_not_merge_states` forces **equal 64-bit hashes**
  (`intern_hashed`, a test seam; the production path is `intern_bytes`
  → `intern_hashed(key, hash(key))`): 600 different keys, all with one hash
  and therefore one probe sequence, get 600 different ids, are found again as
  themselves, and a prefix of a colliding key is a new key. (Before the
  finalization audit this test only produced slot collisions; it now
  collides deliberately.)
* `slot_collisions_do_not_merge_states`: 5 000 keys through table growth.
* `equal_keys_get_one_id_and_different_keys_never_share_one`: prefixes and
  extensions are different keys.
* The interner (`state::Interner`) is a `HashMap<Box<[u8]>, u32>`: lookup by
  `Eq` on the fragment bytes, the hash again only selects a bucket.
* `phase9_storage`: the interned key identifies exactly the states of the
  flat Phase 8 encoding on targeted and random scenarios (BFS, DFS, sleep
  sets).

The canonical encoding itself is unchanged in content. Symmetry reduction,
the knowledge component (orphans, ambiguity, trailer `0xA9`) and pools
(trailer `0xB0`) are part of it. So exploration is identical: the same
states, transitions and verdicts as Phase 8 on all 40 model scenarios of
Phases 6–8 (`reports/phase9/model-identity.txt`, 40 of 40 identical; only
`visitedBytes` differs, by design).

## Measured

`--profile-storage` reports where the memory goes. Workload: the extracted
team-app action with a hypothetical `UNIQUE(teamId, email)`, sleep-set
reduction on (`scripts/phase9-model-bench.py`, `reports/phase9/model-bench.md`).

| actors | stored states | distinct fragments | key arena | key table (incl. arena) | interner | edges | nodes | `visitedBytes` | bytes/state |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 4 | 1 984 | 47 | 66 080 B | 155 648 B | 8 261 B | 40 960 B | 67 584 B | 272 453 B | 137 |
| 6 | 97 408 | 56 | 4 231 104 B | 7 077 888 B | 9 457 B | 2 621 440 B | 4 325 376 B | 14 034 161 B | 144 |

At 6 actors, a state's key is about 43 bytes (43 fragment references of one
byte each). The rest of the per-state cost is the table slots and offsets
(about 30 bytes, sized to powers of two), node records (44 bytes) and edges
(27 bytes).

Phase 8 binary against the final build, same workload, same states and
transitions, one session (peak RSS from `/usr/bin/time -l`; median of 3 runs
at 4–6 actors, 1 run at 7–8; `reports/phase9/final/model-bench.md`):

| actors | states | Phase 8 visited bytes | Phase 9 visited bytes | ratio | Phase 8 peak RSS | Phase 9 peak RSS | ratio |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 4 | 1 984 | 6.67 MB | 0.27 MB | 24× | 17.0 MiB | 9.4 MiB | 1.8× |
| 5 | 13 984 | 56.9 MB | 1.68 MB | 34× | 90.9 MiB | 12.1 MiB | 7.5× |
| 6 | 97 408 | 463 MB | 14.0 MB | 33× | 742.9 MiB | 26.7 MiB | 28× |
| 7 | 669 568 | 3 644 MB | 113 MB | 32× | 2 934.5 MiB | 118.5 MiB | 25× |
| 8 | 4 544 512 | not run | 881 MB | — | not run | 532.4 MiB | — |

The two `visitedBytes` columns use different definitions (Phase 8: key +
96 bytes; Phase 9: everything the table, interner, edges and nodes hold).
Peak RSS is the comparable number. At 8 actors the Phase 8 keys alone would be
about 23.6 GiB (5 442 bytes/state × 4.5 M, extrapolated), so it was not run.
The Phase 9 run took 207 s.

Time: exploration at 7 actors took 25.2 s against 29.0 s for Phase 8, at 6
actors 3.0 s against 3.2 s. A sampling profile (`PERFORMANCE.md` §3) shows
the trade inside: building the key costs 24.5 % of samples instead of
17.8 % (fragment interning adds 6.3 points), the table lookup 1.5 % instead
of 8.7 %. Canonicalization and lookup times are profiler shares, not
instrumented timings.

## Limits

* `maxMemoryBytes` is checked against `visitedBytes`, not RSS. The
  exploration stack and the minimizer are outside it.
* Few distinct fragments is a property of this workload. A model whose
  tables take many distinct values needs more fragments, and keys take two
  bytes per reference once there are 128 or more.
* The table is in memory only. There is no spilling to disk and no lossy
  mode such as bitstate hashing; both were rejected because they would trade
  exactness for capacity.

`test-interleaved.txt` is NOT a baseline. Two `cargo test` runs wrote into one
file: the clean copy of the Phase 11 tree (scratchpad, own target directory) and
an earlier attempt started in the working tree whose `cargo test` child kept
running after its script was stopped (it compiled early Phase 12 edits; see the
doctest errors naming `GeneratorClass`). The baseline was rerun; see
`../test.txt`. Kept only as the record of the mistake.

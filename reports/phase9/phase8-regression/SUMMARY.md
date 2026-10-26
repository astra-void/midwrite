# Phase 8 original-code evidence (3 runs per scenario)

| scenario | model verdict | evidence | runs (followed / matched) | invariant on observed state | crash checks | restarts | effects (model = runtime) | PostgreSQL | s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 17.11 (Homebrew) | 6.6 |
| accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 17.11 (Homebrew) | 6.6 |
| checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 5.9 |
| checkout-idempotent-crash-after-charge | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 17.11 (Homebrew) | 5.8 |
| checkout-idempotent-two-requests | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 17.11 (Homebrew) | 5.5 |
| inventory-notify-after-commit | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | yes | 17.11 (Homebrew) | 5.6 |
| inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 17.11 (Homebrew) | 5.6 |
| inventory-retry-exhausted | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | — | 17.11 (Homebrew) | 5.5 |
| job-claimed-effect-concurrent | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | yes | 17.11 (Homebrew) | 5.7 |
| job-claimed-effect-redelivery | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 17.11 (Homebrew) | 7.7 |
| job-concurrent-claims-duplicate | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 17.11 (Homebrew) | 7.3 |
| job-redelivery-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 5.8 |
| pgjs-accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 17.11 (Homebrew) | 7.0 |
| pgjs-accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 17.11 (Homebrew) | 6.8 |
| pgjs-accept-invitation-twice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | — | 17.11 (Homebrew) | 5.6 |
| pgjs-checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 5.7 |
| pgjs-inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 17.11 (Homebrew) | 5.5 |
| pgjs-invite-double-submit | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | — | 17.11 (Homebrew) | 5.7 |
| shipping-dual-write-crash | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | — | yes | 17.11 (Homebrew) | 4.8 |
| shipping-outbox-no-loss | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 17.11 (Homebrew) | 6.9 |
| shipping-outbox-relay-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 6.8 |

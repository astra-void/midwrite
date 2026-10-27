# Phase 8 original-code evidence (3 runs per scenario)

| scenario | model verdict | evidence | runs (followed / matched) | invariant on observed state | crash checks | restarts | effects (model = runtime) | PostgreSQL | s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 17.11 (Homebrew) | 6.1 |
| accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 17.11 (Homebrew) | 6.2 |
| checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 5.4 |
| checkout-idempotent-crash-after-charge | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 17.11 (Homebrew) | 5.3 |
| checkout-idempotent-two-requests | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 17.11 (Homebrew) | 5.2 |
| inventory-notify-after-commit | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | yes | 17.11 (Homebrew) | 5.1 |
| inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 17.11 (Homebrew) | 5.0 |
| inventory-retry-exhausted | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | — | 17.11 (Homebrew) | 5.0 |
| job-claimed-effect-concurrent | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | yes | 17.11 (Homebrew) | 5.0 |
| job-claimed-effect-redelivery | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 17.11 (Homebrew) | 5.2 |
| job-concurrent-claims-duplicate | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 17.11 (Homebrew) | 5.0 |
| job-redelivery-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 5.3 |
| pgjs-accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 17.11 (Homebrew) | 6.1 |
| pgjs-accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 17.11 (Homebrew) | 6.2 |
| pgjs-accept-invitation-twice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | — | 17.11 (Homebrew) | 5.2 |
| pgjs-checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 5.3 |
| pgjs-inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 17.11 (Homebrew) | 5.1 |
| pgjs-invite-double-submit | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | — | 17.11 (Homebrew) | 5.2 |
| shipping-dual-write-crash | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | — | yes | 17.11 (Homebrew) | 4.4 |
| shipping-outbox-no-loss | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 17.11 (Homebrew) | 6.1 |
| shipping-outbox-relay-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 17.11 (Homebrew) | 6.1 |

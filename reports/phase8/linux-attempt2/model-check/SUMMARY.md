# Phase 8 original-code evidence (3 runs per scenario)

| scenario | model verdict | evidence | runs (followed / matched) | invariant on observed state | crash checks | restarts | effects (model = runtime) | PostgreSQL | s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.7 |
| accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.7 |
| checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.1 |
| checkout-idempotent-crash-after-charge | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.6 |
| checkout-idempotent-two-requests | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.6 |
| inventory-notify-after-commit | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.0 |
| inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.9 |
| inventory-retry-exhausted | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | — | 15.19 (Debian 15.19-0+deb12u1) | 5.6 |
| job-claimed-effect-concurrent | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-invariant-held | 3/3 / 3/3 | holds | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.3 |
| job-claimed-effect-redelivery | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.1 |
| job-concurrent-claims-duplicate | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.7 |
| job-redelivery-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.0 |
| pgjs-accept-crash-lock-holder | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.9 |
| pgjs-accept-crash-while-waiting | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.8 |
| pgjs-accept-invitation-twice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | — | 15.19 (Debian 15.19-0+deb12u1) | 5.2 |
| pgjs-checkout-crash-after-charge | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.1 |
| pgjs-inventory-retry-duplicate-notice | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 5.0 |
| pgjs-invite-double-submit | COUNTEREXAMPLE_FOUND | original-code-reproduced | 3/3 / 3/3 | violated | — | — | — | 15.19 (Debian 15.19-0+deb12u1) | 5.9 |
| shipping-dual-write-crash | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | — | yes | 15.19 (Debian 15.19-0+deb12u1) | 4.3 |
| shipping-outbox-no-loss | NO_COUNTEREXAMPLE_WITHIN_BOUNDS | original-code-recovery-reproduced | 3/3 / 3/3 | holds | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 6.9 |
| shipping-outbox-relay-duplicate | COUNTEREXAMPLE_FOUND | original-code-crash-reproduced | 3/3 / 3/3 | violated | agree | #2 agree | yes | 15.19 (Debian 15.19-0+deb12u1) | 7.1 |

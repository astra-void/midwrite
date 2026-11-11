// Process-lifecycle harness for the source coordinator's own tests
// (crates/midwrite-replay/tests/source_lifecycle.rs). Not an application:
// each entry misbehaves in one way the coordinator must contain.
import { spawn } from "node:child_process";

import { runSourceActor, sourceAttempt, sourceEffect } from "../../../runtime/node/source.js";

const entry = process.env.MIDWRITE_SOURCE_ENTRY;

// A retry whose process dies before it connects.
if (entry === "failRetry" && sourceAttempt() > 1) process.exit(3);

await runSourceActor({
  // Leaves a grandchild in its own session (outside the worker's process
  // group), then returns.
  detach: async () => {
    // The last argument marks the process so the test can look for it.
    const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)", "midwrite-lifecycle-grandchild"], {
      detached: true,
      stdio: "ignore",
    });
    child.unref();
    await sourceEffect("ping", { key: "k" });
    return { child: child.pid };
  },
  // Two effects: a crash can be planned between them.
  twoEffects: async () => {
    await sourceEffect("ping", { key: "k" });
    await sourceEffect("ping", { key: "k" });
    return { done: true };
  },
  failRetry: async () => {
    await sourceEffect("ping", { key: "k" });
    await sourceEffect("ping", { key: "k" });
    return { done: true };
  },
  // Never finishes.
  hang: async () => {
    await sourceEffect("ping", { key: "k" });
    await new Promise(() => {});
  },
  // Burns CPU until RLIMIT_CPU stops it.
  spin: async () => {
    await sourceEffect("ping", { key: "k" });
    for (;;) {}
  },
});

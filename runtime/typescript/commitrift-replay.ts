// CommitRift replay client for TypeScript harnesses (protocol version 1).
//
// Since Phase 4 the implementation lives in one dependency-free ECMAScript
// module, `runtime/node/index.js` (the workspace package
// `@commitrift/replay`), which runs unchanged on Bun and Node.js. This file
// keeps the Phase 3 import path working for Bun harnesses.
//
// See docs/phase3/REPLAY-PROTOCOL.md and docs/phase4/NODE-DRIZZLE.md.

export {
  PROTOCOL_VERSION,
  InjectedFault,
  injectedFaultIn,
  replayActive,
  checkpoint,
  replayDatabase,
  eventLogPath,
  runScenarios,
} from "../node/index.js";
export type { Scenario } from "../node/index.js";

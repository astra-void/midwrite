// AUTHORIZED SOURCE-VERIFICATION HARNESS for the request application.
//
// It calls the application's ORIGINAL exported functions as its server
// would: the services with their inputs, the request handlers with the
// parsed request. The coordinator plays the HTTP client: a handler's
// response is delivered when the coordinator receives it (held at the
// response checkpoint when the model makes the response a separate step).
import * as drizzle from "drizzle-orm";
import * as pgCore from "drizzle-orm/pg-core";
import pg from "pg";
import {
  coordinatePool,
  instrumentDrizzle,
  runSourceActor,
  sourcePoolConfig,
} from "@midwrite/replay/source";

import { usePool } from "../src/db/pool.js";

instrumentDrizzle(drizzle, pgCore);
const pool = coordinatePool(new pg.Pool(sourcePoolConfig({ max: 10 })));
pool.on("error", () => {});
usePool(pool);

const tasks = await import("../src/services/tasks.js");
const signup = await import("../src/handlers/signup.js");
const upserts = await import("../src/services/upserts.js");

await runSourceActor(
  {
    claimTask: (input: { taskId: number; who: string }) => tasks.claimTask(input.taskId, input.who),
    claimTaskAtomic: (input: { taskId: number; who: string }) =>
      tasks.claimTaskAtomic(input.taskId, input.who),
    recordSignup: (input: { accountId: number; requestId: string }) => signup.recordSignup(input),
    recordSignupIdempotent: (input: { accountId: number; requestId: string }) =>
      signup.recordSignupIdempotent(input),
    recordView: (input: { id: number }) => upserts.recordView(input.id),
    reserveHandle: (input: { handle: string; owner: string }) =>
      upserts.reserveHandle(input.handle, input.owner),
    reserveHandleIfFree: (input: { handle: string; owner: string }) =>
      upserts.reserveHandleIfFree(input.handle, input.owner),
  },
  () => pool.end(),
);

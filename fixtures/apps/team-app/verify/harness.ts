// AUTHORIZED SOURCE-VERIFICATION HARNESS for the team application.
//
// It runs the application's ORIGINAL exported Server Action
// (`inviteTeamMember` = validatedActionWithUser(schema, callback)) the way
// the framework would: inside a request with a session, with a FormData
// body. The action, its wrapper and its helpers are imported unchanged.
// What the harness provides is what a framework provides: the database
// pool (coordinated by Midwrite) and the request session.
import * as drizzle from "drizzle-orm";
import * as pgCore from "drizzle-orm/pg-core";
import pg from "pg";
import {
  coordinatePool,
  instrumentDrizzle,
  runSourceActor,
  sourcePoolConfig,
} from "@midwrite/replay/source";

import { usePool } from "../src/lib/db/pool.js";
import { runWithSession } from "../src/lib/auth/session.js";

instrumentDrizzle(drizzle, pgCore);
const pool = coordinatePool(new pg.Pool(sourcePoolConfig({ max: 10 })));
usePool(pool);

// Loaded after the pool is configured (the application's `db` is created
// at module load).
const { acceptInvitation, inviteTeamMember } = await import("../src/app/actions.js");

type Input = { userId: number; form: Record<string, string> };

function formData(fields: Record<string, string>): FormData {
  const data = new FormData();
  for (const [key, value] of Object.entries(fields)) data.set(key, value);
  return data;
}

await runSourceActor(
  {
    inviteTeamMember: (input: Input) =>
      runWithSession({ userId: input.userId }, () => inviteTeamMember({}, formData(input.form))),
    acceptInvitation: (input: Input) =>
      runWithSession({ userId: input.userId }, () => acceptInvitation({}, formData(input.form))),
  },
  () => pool.end(),
);

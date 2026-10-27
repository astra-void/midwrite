// AUTHORIZED SOURCE-VERIFICATION HARNESS for the workspace application.
//
// It runs the application's ORIGINAL exported functions the way the server
// would: the Server Action inside a request with a session, the services
// with their inputs. What the harness provides is what the process entry
// point provides: the database pool (coordinated by CommitRift).
import * as drizzle from "drizzle-orm";
import * as pgCore from "drizzle-orm/pg-core";
import pg from "pg";
import {
  coordinatePool,
  instrumentDrizzle,
  runSourceActor,
  sourcePoolConfig,
} from "@commitrift/replay/source";

import { usePool } from "../src/db/pool.js";
import { runWithSession } from "../src/lib/session.js";

instrumentDrizzle(drizzle, pgCore);
const pool = coordinatePool(new pg.Pool(sourcePoolConfig({ max: 10 })));
// node-postgres emits 'error' on a client whose connection ends; without a
// listener the process dies (Phase 10).
pool.on("error", () => {});
usePool(pool);

// Loaded after the seam is configured (`db` is created at module load).
const actions = await import("../src/app/actions.js");
const workspaces = await import("../src/services/workspaces.js");
const invites = await import("../src/services/invites.js");

await runSourceActor(
  {
    createWorkspaceAction: (input: { email: string; name: string }) =>
      runWithSession({ email: input.email }, () => actions.createWorkspaceAction({ name: input.name })),
    createWorkspace: (input: { ownerEmail: string; name: string }) =>
      workspaces.createWorkspace(input.ownerEmail, input.name),
    createWorkspaceLocked: (input: { ownerEmail: string; name: string }) =>
      workspaces.createWorkspaceLocked(input.ownerEmail, input.name),
    addWorkspace: (input: { ownerId: string; name: string }) =>
      workspaces.addWorkspace(input.ownerId, input.name),
    addWorkspaceIdempotent: (input: { requestId: string; ownerId: string; name: string }) =>
      workspaces.addWorkspaceIdempotent(input.requestId, input.ownerId, input.name),
    registerUser: (input: { email: string; name: string; workspaceName: string }) =>
      workspaces.registerUser(input.email, input.name, input.workspaceName),
    inviteToWorkspace: (input: { workspaceId: string; email: string }) =>
      invites.inviteToWorkspace(input.workspaceId, input.email),
  },
  () => pool.end(),
);

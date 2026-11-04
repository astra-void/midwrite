import { drizzle, type NodePgDatabase } from "drizzle-orm/node-postgres";
import { withReplicas } from "drizzle-orm/pg-core";
import pg from "pg";

import * as schema from "./schema.js";

export type Database = NodePgDatabase<typeof schema>;

const primaryPool = new pg.Pool({ connectionString: process.env.DATABASE_URL });
const replicaPool = new pg.Pool({ connectionString: process.env.REPLICA_DATABASE_URL });

// The primary.
export const db: Database = drizzle(primaryPool, { schema });

// The replica set: select reads from an asynchronous streaming standby that
// may lag; insert, update, delete and transaction run on the primary;
// `$primary` is the primary itself.
export const replicatedDb = withReplicas(db, [drizzle(replicaPool, { schema })]);

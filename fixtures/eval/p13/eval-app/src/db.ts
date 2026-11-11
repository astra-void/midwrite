import { drizzle, type NodePgDatabase } from "drizzle-orm/node-postgres";
import { withReplicas } from "drizzle-orm/pg-core";
import pg from "pg";

import * as schema from "./schema/index.js";

export type Database = NodePgDatabase<typeof schema>;

const primaryPool = new pg.Pool({ connectionString: process.env.DATABASE_URL });
const replicaPool = new pg.Pool({ connectionString: process.env.REPLICA_DATABASE_URL });

// The primary. Most services use it directly.
export const db: Database = drizzle(primaryPool, { schema });

// Profile and label pages go through the replica set: select, selectDistinct,
// $count, with and the relational query API read from the replica (an
// asynchronous streaming standby); insert, update, delete, execute and
// transaction run on the primary; `$primary` is the primary itself.
export const replicatedDb = withReplicas(db, [drizzle(replicaPool, { schema })]);

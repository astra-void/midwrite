import { drizzle, type NodePgDatabase } from "drizzle-orm/node-postgres";
import { withReplicas } from "drizzle-orm/pg-core";
import pg from "pg";

import { createReplicaSet } from "./lib/replicas.js";
import * as schema from "./schema/index.js";

export type Database = NodePgDatabase<typeof schema>;

const primaryPool = new pg.Pool({ connectionString: process.env.DATABASE_URL, max: 20 });
const replicaPool = new pg.Pool({ connectionString: process.env.REPLICA_DATABASE_URL, max: 20 });

// The primary. Most services use it directly.
export const db: Database = drizzle(primaryPool, { schema });

// An asynchronous streaming standby of the primary. It may lag behind the
// primary by an unbounded amount of time.
const replica: Database = drizzle(replicaPool, { schema });

// Drizzle's replica set: select, selectDistinct, $count, with and the
// relational query API read from the replica; insert, update, delete,
// execute and transaction run on the primary; `$primary` is the primary.
export const replicatedDb = withReplicas(db, [replica]);

// The platform team's own replica set (src/lib/replicas.ts), used by the
// settings pages. Declared in commitrift-contracts.json (replicaFactories).
export const readScaled = createReplicaSet(db, [replica]);

// drizzle-orm's own read-replica database (drizzle-orm/pg-core `withReplicas`).
import { drizzle } from "drizzle-orm/node-postgres";
import { withReplicas } from "drizzle-orm/pg-core";
import * as schema from "../db/schema.js";

const primary = drizzle(process.env.DATABASE_PRIMARY_URL!, { schema });
const replica = drizzle(process.env.DATABASE_REPLICA_URL!, { schema });

export const db = withReplicas(primary, [replica]);

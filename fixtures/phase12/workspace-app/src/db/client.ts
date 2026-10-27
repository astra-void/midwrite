import { drizzle, type NodePgDatabase } from "drizzle-orm/node-postgres";

import { configuredPool } from "./pool.js";
import * as schema from "./schema/index.js";

export type Database = NodePgDatabase<typeof schema>;

export const db: Database = drizzle(configuredPool(), { schema });

export function getDb(): Database {
  return db;
}

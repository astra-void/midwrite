import { drizzle, type NodePgDatabase } from "drizzle-orm/node-postgres";

import { configuredPool } from "./pool.js";
import * as schema from "./schema/index.js";

export type Database = NodePgDatabase<typeof schema>;

// Every statement Drizzle sends, with its parameters.
export const sent: { sql: string; params: unknown[] }[] = [];

export const db: Database = drizzle(configuredPool(), {
  schema,
  logger: { logQuery: (sql, params) => void sent.push({ sql, params }) },
});

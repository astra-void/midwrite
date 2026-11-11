import { drizzle, type NodePgDatabase } from "drizzle-orm/node-postgres";
import pg from "pg";

import * as schema from "./schema.js";

export type Database = NodePgDatabase<typeof schema>;

const pool = new pg.Pool({ connectionString: process.env.DATABASE_URL });

export const db: Database = drizzle(pool, { schema });

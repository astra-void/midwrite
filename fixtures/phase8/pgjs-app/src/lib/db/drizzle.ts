// The database client, created at module load from POSTGRES_URL, the way
// saas-starter's lib/db/drizzle.ts does it (postgres-js).
import { drizzle } from "drizzle-orm/postgres-js";
import postgres from "postgres";

import * as schema from "./schema.js";

if (!process.env.POSTGRES_URL) {
  throw new Error("POSTGRES_URL environment variable is not set");
}

export const client = postgres(process.env.POSTGRES_URL);
export const db = drizzle(client, { schema });

// The database client, created at module load from POSTGRES_URL with
// postgres-js (the driver saas-starter uses). The payment functions in
// app/payments.ts are the same as in the node-postgres variant of this app.
import { drizzle } from "drizzle-orm/postgres-js";
import postgres from "postgres";

import * as schema from "./schema.js";

if (!process.env.POSTGRES_URL) {
  throw new Error("POSTGRES_URL environment variable is not set");
}

export const client = postgres(process.env.POSTGRES_URL, { max: 10 });
export const db = drizzle(client, { schema });

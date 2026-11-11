import { sql } from "drizzle-orm";

import { db } from "../db.js";

// E27: runs an operator-supplied maintenance statement.
export async function runMaintenance(statement: string) {
  await db.execute(sql.raw(statement));
}

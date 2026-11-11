import { sql } from "drizzle-orm";

import { db } from "../db.js";

// P14-E37: runs an operator-supplied report statement.
export async function runReport(statement: string) {
  const result = await db.execute(sql.raw(statement));
  return result.rows;
}

// P14-E38: bumps the hit counter of a row of a table named at run time.
export async function bumpCounter(table: string, id: number) {
  await db.execute(sql`update ${sql.identifier(table)} set hits = hits + 1 where id = ${id}`);
}

import { eq, sql } from "drizzle-orm";

import { db } from "../db.js";
import { counters } from "../schema/index.js";

// P15-E28: runs a saved operator query (any SQL text).
export async function runSavedQuery(text: string) {
  const result = await db.execute(sql.raw(text));
  return result.rows;
}

// P15-E29: increments a counter column named at run time.
export async function bumpCounter(column: string, counterId: number) {
  await db.execute(sql`update counters set ${sql.identifier(column)} = ${sql.identifier(column)} + 1 where id = ${counterId}`);
}

// P15-E30: counts a view unless an operator-supplied exclusion query returns
// a row.
export async function countViewUnlessExcluded(counterId: number, exclusionQuery: string) {
  const excluded = await db.execute(sql.raw(exclusionQuery));
  if (excluded.rows.length > 0) return false;
  await db
    .update(counters)
    .set({ views: sql`${counters.views} + 1` })
    .where(eq(counters.id, counterId));
  return true;
}

// Raw SQL is not parsed, so its effects are unknown.
// @expect MW9001 line=7
import { sql } from "drizzle-orm";
import { db } from "@/db";

export async function bumpCounter(id: string) {
  await db.execute(sql`update counters set value = value + 1 where id = ${id}`);
  await queue.publish("counter.bumped", { id });
}

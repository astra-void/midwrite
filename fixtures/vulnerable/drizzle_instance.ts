// A handle created with `drizzle(...)` in the same file is recognized by
// provenance, whatever its name.
// @expect CP1001 line=13 confidence=high
import { drizzle } from "drizzle-orm/node-postgres";
import { Pool } from "pg";
import { orders } from "./schema";

const pg = drizzle(new Pool());

export async function archive(id: string) {
  await pg.delete(orders);
  // The publication below can be lost after the delete commits.
  await queue.publish("order.archived", { id });
}

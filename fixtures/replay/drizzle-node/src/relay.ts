// The outbox relay: publishes every pending outbox row, then marks it
// published. A failure between the two leaves the row pending, so the next
// relay run publishes it AGAIN: delivery is at-least-once, never
// exactly-once. Every event carries the outbox row id so consumers can
// deduplicate.
import { asc, eq, isNull } from "drizzle-orm";

import { db } from "./database.js";
import { queue } from "./queue.js";
import { outbox } from "./schema.js";

export async function relayOutbox() {
  const pending = await db.select().from(outbox).where(isNull(outbox.publishedAt)).orderBy(asc(outbox.id));
  for (const row of pending) {
    await queue.publish(row.topic, row.payload, { id: row.id });
    await db.update(outbox).set({ publishedAt: new Date() }).where(eq(outbox.id, row.id));
  }
}

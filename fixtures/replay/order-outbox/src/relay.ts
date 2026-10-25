// The outbox relay: publishes every pending outbox row, then marks it
// published. A failure between the two leaves the row pending, so the next
// relay run publishes it AGAIN: delivery is at-least-once, never
// exactly-once. Every event carries the outbox row id so that consumers can
// deduplicate.
import { db, eq, isNull } from "../../shared/db.ts";
import { queue } from "../../shared/queue.ts";
import { outbox } from "../../shared/schema.ts";

export async function relayOutbox() {
  const pending = await db.select().from(outbox).where(isNull(outbox.publishedAt));
  for (const row of pending) {
    await queue.publish(String(row.topic), JSON.parse(String(row.payload)), {
      id: `outbox-${row.id}`,
    });
    await db.update(outbox).set({ publishedAt: new Date() }).where(eq(outbox.id, row.id));
  }
}

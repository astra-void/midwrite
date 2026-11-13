import { db } from "@/db";
import { orders } from "@/db/schema";

export function wrap(action: (id: string) => Promise<void>, enabled = true) {
  const send = enabled;
  return async (id: string) => {
    if (send) {
      await db.insert(orders).values({ id });
      await action(id);
    }
  };
}

export async function notify(id: string) { await queue.publish({ id }); }

// @expect MW9001 line=9

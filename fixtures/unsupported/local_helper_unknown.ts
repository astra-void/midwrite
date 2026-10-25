// A same-file helper that performs no recognized operation is not summarized
// (CommitRift cannot tell what its calls do), so calling it after a commit is
// still reported as an unknown effect.
// @expect CP9001 line=15
import { db } from "@/db";
import { orders } from "@/db/schema";
import { cache } from "@/lib/cache";

async function refreshCache(id: string) {
  await cache.invalidate(`order:${id}`);
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  await refreshCache(id);
}

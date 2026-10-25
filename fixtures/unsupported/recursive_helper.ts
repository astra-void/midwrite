// Recursive calls are not summarized (summaries are never inlined into
// themselves): the recursive call stays an unknown effect. The outer call is
// still summarized, because `publishPage` publishes on every path.
// @expect CP9001 line=13 function=publishPage
// @expect CP1001 line=19 confidence=high function=completeBatch
import { db } from "@/db";
import { batches } from "@/db/schema";

async function publishPage(ids: string[], page: number): Promise<void> {
  await queue.publish("batch.page", { ids, page });
  if (ids.length > (page + 1) * 100) {
    // A recursive call: not followed.
    await publishPage(ids, page + 1);
  }
}

export async function completeBatch(id: string, ids: string[]) {
  await db.update(batches).set({ status: "done", id });
  await publishPage(ids, 0);
}

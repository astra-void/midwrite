import { eq } from "drizzle-orm";

import { replicatedDb } from "../db.js";
import { pageViews } from "../schema.js";

// G4 (no declared property): counts a page view by reading the counter
// through the replica set and writing the incremented value to the
// primary. Under the modeled asynchronous replica a read may be stale even
// when the requests run one after the other, so a final count of 1 after
// two views and a final count of 2 are both outcomes of serial executions:
// without a declared freshness promise neither is reported.
export async function recordPageView(pageId: number) {
  const [page] = await replicatedDb
    .select({ views: pageViews.views })
    .from(pageViews)
    .where(eq(pageViews.pageId, pageId))
    .limit(1);
  if (!page) {
    throw new Error("page not found");
  }
  await replicatedDb
    .update(pageViews)
    .set({ views: page.views + 1 })
    .where(eq(pageViews.pageId, pageId));
}

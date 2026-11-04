// The caller passes the root handle `db` inside its own transaction: the
// helper's write commits on its own (MW1004, a specific bypass: warning under
// the default policy).
// @expect MW1004 line=11 confidence=high function=checkout
import { db } from "@/db";
import { payments } from "@/db/schema";
import { saveOrder } from "./repository";

export async function checkout(id: string) {
  await db.transaction(async (tx) => {
    await saveOrder(db, id);
    await tx.insert(payments).values({ id });
  });
}

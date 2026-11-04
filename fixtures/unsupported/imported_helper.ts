// Only functions declared in the same file are summarized. A helper imported
// from another module stays an unknown effect, reported because it runs after
// a recognized commit.
// @expect MW9001 line=11
import { db } from "@/db";
import { orders } from "@/db/schema";
import { notify } from "@/lib/notify";

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  await notify(id);
}

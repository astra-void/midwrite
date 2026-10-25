// Labeled break/continue is not modeled; paths through it are cut and reported.
// @expect CP1001 line=11 confidence=medium
// @expect CP9001 line=13
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function sweep(batches: string[][]) {
  outer: for (const batch of batches) {
    for (const id of batch) {
      await db.update(orders).set({ status: "swept" });
      await queue.publish("order.swept", { id });
      if (id === "stop") {
        break outer;
      }
    }
  }
}

// A switch case that falls through into the next case.
// @expect CP1001 line=12 confidence=high
import { db } from "@/db";
import { orders } from "@/db/schema";

export async function transition(id: string, event: "pay" | "ship") {
  switch (event) {
    case "pay":
      await db.update(orders).set({ status: "paid" });
    // falls through
    case "ship":
      await queue.publish("order.shipped", { id });
      break;
  }
}

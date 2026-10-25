// @expect none
import { orders } from "@/db/schema";

export async function saveOrder(executor: Executor, id: string) {
  await executor.update(orders).set({ status: "paid" });
}

// @expect none
import { updateOrder } from "./repository";

export async function completeOrder(id: string) {
  await updateOrder(id, "complete");
}

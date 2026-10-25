// @expect none
export async function publishOrder(id: string) {
  await queue.publish("order", { id });
}
export type OrderEvent = { id: string };

// @expect none
export async function announce(id: string) {
  await queue.publish("order.closed", { id });
}

// @expect none
export async function maybeNotify(id: string, enabled: boolean) {
  if (enabled) {
    await queue.publish("order.paid", { id });
  }
}

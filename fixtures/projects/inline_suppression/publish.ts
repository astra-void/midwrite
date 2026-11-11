// @expect none
export async function notify() {
  // midwrite-ignore MW1001: applies here, not to callers
  await queue.publish("order.completed", {});
}

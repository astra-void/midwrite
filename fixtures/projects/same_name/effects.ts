// @expect none
export async function notify(id: string) {
  await queue.publish("notified", { id });
}

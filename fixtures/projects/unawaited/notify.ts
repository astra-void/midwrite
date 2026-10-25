// @expect none
export async function announce(id: string) {
  await queue.publish("x", { id });
}

// @expect none
export async function send(id: string) {
  await queue.publish("one", { id });
}

// @expect none
export async function send(id: string) {
  await mailer.send({ id });
}

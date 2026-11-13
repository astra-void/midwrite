// Deliberately precedes the factory module in the project function table.
// @expect none
export async function unrelated() {
  await mailer.send({});
}

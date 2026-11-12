// @expect none
import * as repository from "./helper";
export async function submit(id: string) {
  await db.transaction(async tx => { await repository.replace(tx, id); });
}

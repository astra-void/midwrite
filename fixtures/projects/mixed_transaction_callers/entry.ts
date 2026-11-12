// @expect none
import { replace } from "./helper";
export async function transactional() {
  await db.transaction(async tx => { await replace(tx); });
}
export async function direct() { await replace(db); }

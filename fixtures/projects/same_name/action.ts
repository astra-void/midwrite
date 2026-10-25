// Two modules export `notify`; only the imported one is used. It performs
// no recognized operation, so no CP1001 is claimed.
// @expect none
import { notify } from "./quiet";

export async function start(id: string) {
  await notify(id);
  await db.update(orders).set({ status: "started" });
}

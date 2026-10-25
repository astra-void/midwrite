// `announce(id)` is neither awaited nor returned: not summarized, silent.
// @expect none
import { announce } from "./notify";

export async function act(id: string) {
  await db.update(orders).set({ status: "x" });
  announce(id);
}

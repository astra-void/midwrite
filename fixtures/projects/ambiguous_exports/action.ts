// `send` is provided by two `export *` modules: CommitRift does not choose.
// @expect CP9001 line=7 function=run
import { send } from "./index";

export async function run(id: string) {
  await db.update(orders).set({ status: "x" });
  await send(id);
}

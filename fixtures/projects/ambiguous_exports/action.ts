// `send` is provided by two `export *` modules: Midwrite does not choose.
// @expect MW9001 line=7 function=run
import { send } from "./index";

export async function run(id: string) {
  await db.update(orders).set({ status: "x" });
  await send(id);
}

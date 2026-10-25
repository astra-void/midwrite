// `ping` and `pong` call each other across modules. Each is summarized into
// the other once; the call that would re-enter the function being analyzed
// is recursive and stays unknown (not reported: within the inlined call it
// follows the callee's own publish). In `ping`, the insert commits before
// `pong` publishes.
// @expect CP1001 line=12 confidence=high function=ping
import { db } from "@/db";
import { pong } from "./b";

export async function ping(n: number) {
  await db.insert(log).values({ n });
  await pong(n - 1);
}

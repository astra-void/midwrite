// `pong` publishes, then (only when n > 0) `ping` inserts.
// @expect MW1002 line=6 confidence=medium function=pong
import { ping } from "./a";

export async function pong(n: number) {
  await queue.publish("pong", { n });
  if (n > 0) {
    await ping(n);
  }
}

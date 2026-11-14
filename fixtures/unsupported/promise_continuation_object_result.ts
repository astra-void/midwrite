// DEV-172, not correlated: a handler that returns an object literal. The
// property the caller tests (`result.ok`) is not carried from the
// handler's return, so the handler's insert is still paired with the
// publication after the early return (a false positive, at low
// confidence). Return a literal (`null`, `false`) to keep the two apart.
// @expect MW1001 line=18 confidence=low function=resultObject
import { db } from "@/db";
import { events } from "@/db/schema";

export async function resultObject(id: string) {
  const result = await loadUser(id)
    .then(() => ({ ok: true as const }))
    .catch(async () => {
      await db.insert(events).values({ id });
      return { ok: false as const };
    });
  if (!result.ok) return;
  await queue.publish("user.loaded", { id });
}

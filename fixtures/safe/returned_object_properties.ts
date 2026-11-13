// A property of the object a helper returns decides a later branch of the
// caller, through a spread (`{ ...r, consumed: true }`), a helper that
// returns its argument with an extra property, a returned call
// (`return resolveSlack(...)`), a type assertion and a truthiness test. The
// path that inserts the identity always returns `consumed: true`, and the
// caller returns early on it, so no path commits both writes. The object
// escapes only after the test.
// @expect none
import { db } from "@/db";
import { identities, metadata } from "@/db/schema";

type Resolved = { connected: boolean; consumed?: boolean; teamId?: string };

function consume(resolved: { connected: true; teamId: string }) {
  return { ...resolved, consumed: true as const };
}

async function resolveSlack(link: boolean) {
  if (link) {
    await db.insert(identities).values({ id: "1" });
    return consume({ connected: true as const, teamId: "t" });
  }
  return { connected: false as const };
}

async function resolveConversation(platform: string, link: boolean) {
  if (platform === "cached") {
    return { connected: true as const, teamId: "t" };
  }
  return resolveSlack(link);
}

export async function handle(
  platform: string,
  link: boolean,
  keep: (r: Resolved) => Promise<void>,
) {
  const resolved = (await resolveConversation(platform, link)) as Resolved | null;
  if (!resolved || resolved.connected === false) return;
  if (resolved.consumed) return;
  await keep(resolved);
  await db.update(metadata).set({ seen: 1 });
}

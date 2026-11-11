import { and, eq } from "drizzle-orm";
import { z } from "zod";

import { db } from "../../../db.js";
import { waitlistEntries } from "../../../schema/index.js";

const JoinWaitlistBody = z.object({
  eventId: z.number().int().positive(),
  email: z.email(),
  name: z.string().trim().max(80).optional(),
});

// P16-E17: POST /api/waitlist — puts an email address on an event's
// waitlist, once.
export async function POST(request: Request) {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return Response.json({ error: "body must be JSON" }, { status: 400 });
  }
  const parsed = JoinWaitlistBody.safeParse(body);
  if (!parsed.success) {
    return Response.json({ error: z.treeifyError(parsed.error) }, { status: 422 });
  }
  const { eventId, email, name } = parsed.data;

  const [existing] = await db
    .select({ id: waitlistEntries.id })
    .from(waitlistEntries)
    .where(and(eq(waitlistEntries.eventId, eventId), eq(waitlistEntries.email, email)))
    .limit(1);
  if (existing) {
    return Response.json({ status: "already-joined", id: existing.id });
  }

  const [entry] = await db
    .insert(waitlistEntries)
    .values({ eventId, email, name: name ?? null })
    .returning({ id: waitlistEntries.id });
  return Response.json({ status: "joined", id: entry?.id }, { status: 201 });
}

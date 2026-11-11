import { and, eq } from "drizzle-orm";
import { z } from "zod";

import { db } from "../../../db.js";
import { rsvps } from "../../../schema/index.js";

const RsvpBody = z.object({
  eventId: z.number().int().positive(),
  email: z.email(),
  name: z.string().trim().max(80).optional(),
});

// P16-E18: POST /api/rsvp — registers an email address for an event, once.
export async function POST(request: Request) {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return Response.json({ error: "body must be JSON" }, { status: 400 });
  }
  const parsed = RsvpBody.safeParse(body);
  if (!parsed.success) {
    return Response.json({ error: z.treeifyError(parsed.error) }, { status: 422 });
  }
  const { eventId, email, name } = parsed.data;
  const sameRsvp = and(eq(rsvps.eventId, eventId), eq(rsvps.email, email));

  const [existing] = await db.select({ id: rsvps.id }).from(rsvps).where(sameRsvp).limit(1);
  if (existing) {
    return Response.json({ status: "already-registered", id: existing.id });
  }

  const [entry] = await db
    .insert(rsvps)
    .values({ eventId, email, name: name ?? null })
    .onConflictDoNothing()
    .returning({ id: rsvps.id });
  if (!entry) {
    const [winner] = await db.select({ id: rsvps.id }).from(rsvps).where(sameRsvp).limit(1);
    return Response.json({ status: "already-registered", id: winner?.id });
  }
  return Response.json({ status: "registered", id: entry.id }, { status: 201 });
}

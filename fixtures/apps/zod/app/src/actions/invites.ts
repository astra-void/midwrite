import { and, eq } from "drizzle-orm";
import { z } from "zod";

import { db } from "../db/client.js";
import { invites, members } from "../db/schema.js";

const ROLES = ["member", "admin"] as const;

// zod 4 (package.json pins 4.6.5): unknown keys are rejected, the team is a
// RFC 9562 UUID, the address must match zod 4's email pattern, the role
// defaults to "member", the note may be absent, undefined or null.
const inviteInput = z.strictObject({
  teamId: z.uuid(),
  email: z.email(),
  role: z.enum(ROLES).default("member"),
  note: z.string().max(200).nullish(),
});

// Check-then-insert without a unique key: two concurrent invitations of
// one address both insert.
export async function inviteMember(input: unknown) {
  const data = inviteInput.parse(input);
  const [existing] = await db
    .select({ id: invites.id })
    .from(invites)
    .where(and(eq(invites.teamId, data.teamId), eq(invites.email, data.email)))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(invites)
    .values({ teamId: data.teamId, email: data.email, role: data.role, note: data.note ?? null })
    .returning({ id: invites.id });
  return created.id;
}

const memberInput = z.strictObject({
  teamId: z.uuid(),
  email: z.email(),
  role: z.enum(ROLES).default("member"),
});

// The same against UNIQUE(team_id, email): the second insert does nothing.
export async function addMember(input: unknown) {
  const parsed = memberInput.safeParse(input);
  if (!parsed.success) return null;
  const data = parsed.data;
  const [existing] = await db
    .select({ id: members.id })
    .from(members)
    .where(and(eq(members.teamId, data.teamId), eq(members.email, data.email)))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(members)
    .values({ teamId: data.teamId, email: data.email, role: data.role })
    .onConflictDoNothing()
    .returning({ id: members.id });
  return created?.id ?? null;
}

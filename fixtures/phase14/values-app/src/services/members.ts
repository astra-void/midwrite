import { and, eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { audit, members } from "../db/schema.js";

export interface GrantInput {
  teamId: number;
  memberId: number;
  email: string;
  roles: string[];
}

// Only an admin may grant; the check-then-insert is not atomic.
export async function grantMembership(input: GrantInput) {
  if (!input.roles.includes("admin")) throw new Error("forbidden");
  const email = input.email.trim().toLowerCase();
  const [existing] = await db
    .select()
    .from(members)
    .where(and(eq(members.teamId, input.teamId), eq(members.email, email)))
    .limit(1);
  if (existing) return existing.id;
  await db.insert(members).values({
    id: input.memberId,
    teamId: input.teamId,
    email,
    role: input.roles.at(-1) ?? "member",
  });
  return input.memberId;
}

// Several invitations at once: each normalized email once (a Set of the
// emails already handled), each checked then inserted.
export async function inviteAll(teamId: number, firstId: number, emails: string[]) {
  const seen = new Set<string>();
  let next = firstId;
  for (const raw of emails) {
    const email = raw.trim().toLowerCase();
    if (seen.has(email)) continue;
    seen.add(email);
    const [existing] = await db
      .select()
      .from(members)
      .where(and(eq(members.teamId, teamId), eq(members.email, email)))
      .limit(1);
    if (!existing) {
      await db.insert(members).values({ id: next, teamId, email, role: "member" });
      next = next + 1;
    }
  }
  return seen.size;
}

// A note is optional: `undefined` and `null` are different inputs.
export async function recordNote(id: number, note?: string | null) {
  if (note === undefined) return "skipped";
  await db.insert(audit).values({ id, note: note ?? "(cleared)" });
  return note === null ? "cleared" : "written";
}

// Counts per role with a Map that never leaves the function.
export async function roleSummary(teamId: number, roles: string[]) {
  const counts = new Map<string, number>();
  for (const r of roles) {
    counts.set(r, (counts.get(r) ?? 0) + 1);
  }
  const admins = counts.get("admin") ?? 0;
  if (admins > 1) {
    await db.insert(audit).values({ id: teamId, note: `admins:${admins}` });
  }
  return counts.size;
}

// Patterns that stay unsupported: array mutation, an escaping Map, a
// callback that calls a function.
export async function unsupportedShapes(teamId: number, emails: string[]) {
  const list = [...emails];
  list.push("x@example.test");
  const byEmail = new Map<string, number>();
  register(byEmail);
  byEmail.set("a", 1);
  const checked = emails.filter((e) => isAllowed(e));
  await db.insert(audit).values({ id: teamId, note: checked.join(",") });
}

function register(m: Map<string, number>) {
  return m;
}

function isAllowed(e: string) {
  return e.endsWith("@example.test");
}

// Iterating a Set is not modeled (its items are not indexed).
export async function auditEach(ids: number[]) {
  const unique = new Set<number>(ids);
  for (const id of unique) {
    await db.insert(audit).values({ id, note: "seen" });
  }
}

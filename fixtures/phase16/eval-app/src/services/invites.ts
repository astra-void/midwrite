import { and, eq } from "drizzle-orm";
import { z } from "zod";

import { db } from "../db.js";
import { projectInvites, workspaceInvites } from "../schema/index.js";

export type InviteResult = "invited" | "already-invited";

// projectId is required and may be null: null invites to the whole
// workspace.
const InviteMemberInput = z.strictObject({
  workspaceId: z.number().int().positive(),
  email: z.email(),
  projectId: z.number().int().positive().nullable(),
});

// projectId is required and is never null.
const InviteProjectMemberInput = z.strictObject({
  workspaceId: z.number().int().positive(),
  email: z.email(),
  projectId: z.number().int().positive(),
});

async function recordInvite(workspaceId: number, email: string, projectId: number | null): Promise<InviteResult> {
  if (projectId === null) {
    const [pending] = await db
      .select({ id: workspaceInvites.id })
      .from(workspaceInvites)
      .where(and(eq(workspaceInvites.workspaceId, workspaceId), eq(workspaceInvites.email, email)))
      .limit(1);
    if (pending) return "already-invited";
    await db.insert(workspaceInvites).values({ workspaceId, email });
    return "invited";
  }
  const inserted = await db
    .insert(projectInvites)
    .values({ projectId, email })
    .onConflictDoNothing()
    .returning({ projectId: projectInvites.projectId });
  return inserted.length > 0 ? "invited" : "already-invited";
}

// P16-E05: invites an email address to the workspace (projectId null) or to
// one of its projects.
export async function inviteMember(raw: unknown): Promise<InviteResult> {
  const input = InviteMemberInput.parse(raw);
  return recordInvite(input.workspaceId, input.email, input.projectId);
}

// P16-E06: invites an email address to one project of the workspace.
export async function inviteProjectMember(raw: unknown): Promise<InviteResult> {
  const input = InviteProjectMemberInput.parse(raw);
  return recordInvite(input.workspaceId, input.email, input.projectId);
}

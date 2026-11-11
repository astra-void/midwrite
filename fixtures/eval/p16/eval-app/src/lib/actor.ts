import { z } from "zod";

// The caller, as decoded from the signed session claims by the request
// middleware and validated again here. Unknown keys are rejected.
export const ActorSchema = z.strictObject({
  accountId: z.uuid(),
  workspaceId: z.number().int().positive(),
  role: z.enum(["owner", "admin", "approver", "member"]),
});

export type Actor = z.infer<typeof ActorSchema>;
export type Role = Actor["role"];

export class ForbiddenError extends Error {}
export class NotFoundError extends Error {}

export function requireRole(actor: Actor, ...allowed: Role[]): void {
  if (!allowed.includes(actor.role)) {
    throw new ForbiddenError(`role ${actor.role} may not perform this action`);
  }
}

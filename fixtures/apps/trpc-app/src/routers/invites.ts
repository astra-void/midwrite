import { and, eq } from "drizzle-orm";
import { z } from "zod";

import { invites } from "../db/schema.js";
import { adminProcedure, createTRPCRouter, protectedProcedure } from "../trpc.js";

const inviteInput = z.strictObject({
  email: z.email(),
  role: z.enum(["member", "admin"]).default("member"),
});

export const invitesRouter = createTRPCRouter({
  // Check-then-insert without a unique key: two concurrent invitations of
  // one address both insert.
  invite: protectedProcedure.input(inviteInput).mutation(async ({ ctx, input }) => {
    const [existing] = await ctx.db
      .select({ id: invites.id })
      .from(invites)
      .where(and(eq(invites.teamId, ctx.teamId), eq(invites.email, input.email)))
      .limit(1);
    if (existing) return existing.id;
    const [created] = await ctx.db
      .insert(invites)
      .values({ teamId: ctx.teamId, email: input.email, role: input.role })
      .returning({ id: invites.id });
    return created.id;
  }),

  // The same, with the context destructured in the parameter.
  inviteNested: protectedProcedure
    .input(inviteInput)
    .mutation(async ({ ctx: { db, teamId }, input }) => {
      const [existing] = await db
        .select({ id: invites.id })
        .from(invites)
        .where(and(eq(invites.teamId, teamId), eq(invites.email, input.email)))
        .limit(1);
      if (existing) return existing.id;
      const [created] = await db
        .insert(invites)
        .values({ teamId, email: input.email, role: input.role })
        .returning({ id: invites.id });
      return created.id;
    }),

  // A parameter that is not destructured: not modeled.
  inviteByOpts: protectedProcedure.input(inviteInput).mutation(async (opts) => {
    await opts.ctx.db.insert(invites).values({
      teamId: opts.ctx.teamId,
      email: opts.input.email,
      role: opts.input.role,
    });
  }),

  // A builder midwrite-contracts.json does not declare.
  adminInvite: adminProcedure.input(inviteInput).mutation(async ({ ctx, input }) => {
    await ctx.db.insert(invites).values({ teamId: ctx.teamId, email: input.email, role: input.role });
  }),
});

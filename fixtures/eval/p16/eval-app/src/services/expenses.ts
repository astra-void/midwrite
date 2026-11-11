import { and, eq } from "drizzle-orm";
import { z } from "zod";

import { db } from "../db.js";
import { ActorSchema, ForbiddenError, NotFoundError, requireRole } from "../lib/actor.js";
import { expenses, reimbursements } from "../schema/index.js";

const ApproveExpenseInput = z.strictObject({
  actor: ActorSchema,
  expenseId: z.number().int().positive(),
});

// P16-E07: an owner or approver of the workspace approves a submitted
// expense; approval schedules its reimbursement.
export async function approveExpense(raw: unknown) {
  const { actor, expenseId } = ApproveExpenseInput.parse(raw);
  requireRole(actor, "owner", "approver");

  const [expense] = await db
    .select({ status: expenses.status, amountCents: expenses.amountCents, submittedBy: expenses.submittedBy })
    .from(expenses)
    .where(and(eq(expenses.id, expenseId), eq(expenses.workspaceId, actor.workspaceId)))
    .limit(1);
  if (!expense) throw new NotFoundError(`expense ${expenseId}`);
  if (expense.submittedBy === actor.accountId) throw new ForbiddenError("cannot approve your own expense");
  if (expense.status !== "submitted") return { status: expense.status };

  await db.update(expenses).set({ status: "approved", approvedBy: actor.accountId }).where(eq(expenses.id, expenseId));
  await db.insert(reimbursements).values({ expenseId, amountCents: expense.amountCents });
  return { status: "approved" as const };
}

// P16-E08: the same; the transition is one conditional UPDATE in the
// transaction that schedules the reimbursement.
export async function approveExpenseOnce(raw: unknown) {
  const { actor, expenseId } = ApproveExpenseInput.parse(raw);
  requireRole(actor, "owner", "approver");

  return db.transaction(async (tx) => {
    const [expense] = await tx
      .select({ status: expenses.status, amountCents: expenses.amountCents, submittedBy: expenses.submittedBy })
      .from(expenses)
      .where(and(eq(expenses.id, expenseId), eq(expenses.workspaceId, actor.workspaceId)))
      .limit(1);
    if (!expense) throw new NotFoundError(`expense ${expenseId}`);
    if (expense.submittedBy === actor.accountId) throw new ForbiddenError("cannot approve your own expense");
    if (expense.status !== "submitted") return { status: expense.status };

    const approved = await tx
      .update(expenses)
      .set({ status: "approved", approvedBy: actor.accountId })
      .where(and(eq(expenses.id, expenseId), eq(expenses.status, "submitted")))
      .returning({ amountCents: expenses.amountCents });
    if (approved.length === 0) return { status: "approved" as const };

    await tx.insert(reimbursements).values({ expenseId, amountCents: expense.amountCents });
    return { status: "approved" as const };
  });
}

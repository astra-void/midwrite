// Back-office services: task claims, bank-feed payments, manager shifts.
import { and, count, eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { shifts, taskAssignments, tasks } from "../db/schema.js";
import { insertPayment, insertSettledPayment } from "../repositories/payment-repository.js";

export interface ClaimRequest {
  taskId: number;
  assignee: string;
}

/** Reads the task, and if it is open marks it claimed and assigns it. */
export async function claimTask(input: ClaimRequest) {
  const [task] = await db.select().from(tasks).where(eq(tasks.id, input.taskId)).limit(1);
  if (!task || task.status !== "open") {
    return { claimed: false };
  }
  await db.update(tasks).set({ status: "claimed" }).where(eq(tasks.id, task.id));
  await db.insert(taskAssignments).values({ taskId: task.id, assignee: input.assignee });
  return { claimed: true };
}

/** Claims with a conditional update and checks that a row changed. */
export async function claimTaskConditional(input: ClaimRequest) {
  const changed = await db
    .update(tasks)
    .set({ status: "claimed" })
    .where(and(eq(tasks.id, input.taskId), eq(tasks.status, "open")))
    .returning();
  if (changed.length === 0) {
    return { claimed: false };
  }
  await db.insert(taskAssignments).values({ taskId: input.taskId, assignee: input.assignee });
  return { claimed: true };
}

export interface BankTransfer {
  orderId: number;
  reference: string;
  amount: number;
}

/** Records a transfer from the bank feed; a failed insert is tried once more. */
export async function recordPayment(transfer: BankTransfer) {
  try {
    await insertPayment(transfer.orderId, transfer.reference, transfer.amount);
  } catch {
    await insertPayment(transfer.orderId, transfer.reference, transfer.amount);
  }
  return { recorded: true };
}

/** The same retry against the ledger keyed by the transfer reference. */
export async function recordPaymentIdempotent(transfer: BankTransfer) {
  try {
    await insertSettledPayment(transfer.orderId, transfer.reference, transfer.amount);
  } catch {
    await insertSettledPayment(transfer.orderId, transfer.reference, transfer.amount);
  }
  return { recorded: true };
}

export interface ShiftChange {
  storeId: number;
  shiftId: number;
}

/** A manager goes off duty if another manager of the store is still on duty. */
export async function endShift(change: ShiftChange) {
  return db.transaction(
    async (tx) => {
      const [onDuty] = await tx
        .select({ value: count() })
        .from(shifts)
        .where(and(eq(shifts.storeId, change.storeId), eq(shifts.onDuty, true)));
      if (onDuty.value < 2) {
        throw new Error("the last manager on duty cannot leave");
      }
      await tx.update(shifts).set({ onDuty: false }).where(eq(shifts.id, change.shiftId));
      return { ended: true };
    },
    { isolationLevel: "repeatable read" },
  );
}

/** The same check under SERIALIZABLE. */
export async function endShiftSerializable(change: ShiftChange) {
  return db.transaction(
    async (tx) => {
      const [onDuty] = await tx
        .select({ value: count() })
        .from(shifts)
        .where(and(eq(shifts.storeId, change.storeId), eq(shifts.onDuty, true)));
      if (onDuty.value < 2) {
        throw new Error("the last manager on duty cannot leave");
      }
      await tx.update(shifts).set({ onDuty: false }).where(eq(shifts.id, change.shiftId));
      return { ended: true };
    },
    { isolationLevel: "serializable" },
  );
}

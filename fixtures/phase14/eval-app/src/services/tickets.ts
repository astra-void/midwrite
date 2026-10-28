import { and, eq, inArray } from "drizzle-orm";

import { db } from "../db.js";
import { ticketEvents, tickets } from "../schema/index.js";

// Ticket states an agent may claim from.
const CLAIMABLE: string[] = ["open", "reopened"];

// P14-E03: reads the ticket, assigns it if its state is claimable, records
// the assignment.
export async function claimTicket(ticketId: number, agentId: string) {
  const [ticket] = await db.select().from(tickets).where(eq(tickets.id, ticketId)).limit(1);
  if (!ticket || !CLAIMABLE.includes(ticket.status)) return false;
  await db.update(tickets).set({ status: "assigned", assigneeId: agentId }).where(eq(tickets.id, ticketId));
  await db.insert(ticketEvents).values({ ticketId, kind: "assigned", actor: agentId });
  return true;
}

// P14-E04: the claimable states go into the UPDATE's WHERE clause; the
// assignment is recorded only by the caller whose UPDATE matched.
export async function claimTicketAtomic(ticketId: number, agentId: string) {
  const [claimed] = await db
    .update(tickets)
    .set({ status: "assigned", assigneeId: agentId })
    .where(and(eq(tickets.id, ticketId), inArray(tickets.status, CLAIMABLE)))
    .returning({ id: tickets.id });
  if (!claimed) return false;
  await db.insert(ticketEvents).values({ ticketId, kind: "assigned", actor: agentId });
  return true;
}

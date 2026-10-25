// A Next.js route handler in a .tsx file next to a component.
// @expect CP1001 line=10 confidence=high function=POST
import { db } from "@/db";
import { orders } from "@/db/schema";

export const POST = async (request: Request) => {
  const { id } = await request.json();
  await db.update(orders).set({ status: "paid" });

  await queue.publish("order.completed", { id });
  return Response.json({ ok: true });
};

export function OrderBadge({ status }: { status: string }) {
  return <span className="badge">{status}</span>;
}

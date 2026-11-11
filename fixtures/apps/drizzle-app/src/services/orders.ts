import { db } from "../db/client.js";
import { counters, orderItems, orders, postTags } from "../db/schema.js";

// An order and its two lines in one transaction: the lines are one
// multi-row INSERT (both foreign keys are checked per row).
export async function placeOrder(orderId: number, customer: string, skuA: string, skuB: string) {
  return await db.transaction(async (tx) => {
    await tx.insert(orders).values({ id: orderId, customer });
    const lines = await tx
      .insert(orderItems)
      .values([
        { orderId, sku: skuA, qty: 1 },
        { orderId, sku: skuB, qty: 2 },
      ])
      .returning();
    return lines.length;
  });
}

// Two tags at once, skipping tags the post already has.
export async function addTags(postId: number, a: string, b: string) {
  const added = await db
    .insert(postTags)
    .values([
      { postId, tag: a },
      { postId, tag: b },
    ])
    .onConflictDoNothing()
    .returning({ tag: postTags.tag });
  return added.length;
}

// Two tags at once; a tag the post already has fails the whole statement.
export async function addTagsStrict(postId: number, a: string, b: string) {
  await db.insert(postTags).values([
    { postId, tag: a },
    { postId, tag: b },
  ]);
}

// Two counters bumped by one statement.
export async function bumpCounters(a: string, b: string) {
  await db
    .insert(counters)
    .values([
      { name: a, hits: 1 },
      { name: b, hits: 1 },
    ])
    .onConflictDoUpdate({
      target: counters.name,
      set: { hits: 7, updatedAt: new Date() },
    });
}

// Writes reached through higher-order helpers, nested in each other. Each
// is followed by a message publish.
import { db } from "@/db";
import { events, orders } from "@/db/schema";
import { inTransaction, retryForever, withRetry, withSpan } from "./wrappers";

declare const queue: { publish(message: unknown): Promise<void> };

// The same helper nested in itself, with a different callback each time.
export async function retryTwice(id: string) {
  await withRetry(() => withRetry(() => db.insert(orders).values({ id })));
  await queue.publish({ id });
}

// Two different helpers nested.
export async function tracedRetry(id: string) {
  await withSpan("save", () => withRetry(() => db.insert(orders).values({ id })));
  await queue.publish({ id });
}

// Three levels of helpers, then the write.
export async function deeplyWrapped(id: string) {
  await withSpan("outer", () =>
    withSpan("inner", () => withRetry(() => db.insert(orders).values({ id }))),
  );
  await queue.publish({ id });
}

// Both writes inside the transaction helper, reached through another
// helper: atomic, only the publish after the commit is reported.
export async function tracedTransaction(id: string) {
  await withSpan("tx", () =>
    inTransaction(async (tx) => {
      await tx.insert(orders).values({ id });
      await tx.insert(events).values({ id });
    }),
  );
  await queue.publish({ id });
}

// A recursive helper: its first run of the callback is summarized, the
// recursive call is not.
export async function recursive(id: string) {
  await db.insert(events).values({ id });
  await retryForever(() => db.insert(orders).values({ id }));
}

// @expect MW1001 line=12 confidence=medium function=retryTwice
// @expect MW1001 line=18 confidence=medium function=tracedRetry
// @expect MW1001 line=26 confidence=medium function=deeplyWrapped
// @expect MW1001 line=38 confidence=high function=tracedTransaction
// @expect MW1004 line=45 confidence=medium function=recursive
// @expect MW9001 line=45 function=recursive

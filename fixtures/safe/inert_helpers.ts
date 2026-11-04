// Phase 5: calls of functions proven inert (no write, transaction, external
// effect or unknown call on any path, including the callbacks they run) are
// resolved, not unknown: no MW9001 after the commit. `withTimeout` only
// uses modeled builtins (`new Promise`, `setTimeout`, `Promise.race`,
// `clearTimeout`) and awaits its argument, a builtin here.
// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

async function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error("timeout")), ms);
  });
  try {
    return await Promise.race([promise, timeout]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

async function formatReceipt(id: string) {
  // `part.toUpperCase()` would not be provable: without types, the element
  // of `[id, ...]` could be anything. `join` on an array literal is modeled.
  const parts = [id, "receipt"];
  return parts.join("-");
}

function factorial(n: number): number {
  return n <= 1 ? 1 : n * factorial(n - 1);
}

export async function completeOrder(id: string) {
  await db.update(orders).set({ status: "paid" });
  await withTimeout(Promise.resolve(factorial(3)), 1000);
  await formatReceipt(id);
}

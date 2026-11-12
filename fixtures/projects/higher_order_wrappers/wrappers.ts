// @expect none
import { db } from "@/db";

// Higher-order helpers that run the callback they are given.
export async function withRetry<T>(fn: () => Promise<T>, attempts = 3): Promise<T> {
  for (let i = 0; ; i++) {
    try {
      return await fn();
    } catch (error) {
      if (i + 1 >= attempts) throw error;
    }
  }
}

export async function withSpan<T>(name: string, fn: () => Promise<T>): Promise<T> {
  console.log("start", name);
  const result = await fn();
  console.log("end", name);
  return result;
}

export async function inTransaction<T>(fn: (tx: typeof db) => Promise<T>): Promise<T> {
  return db.transaction((tx) => fn(tx));
}

// Runs the callback, and on failure calls itself with a new callback that
// wraps it: genuinely recursive, so the recursive call is not summarized.
export async function retryForever<T>(fn: () => Promise<T>): Promise<T> {
  try {
    return await fn();
  } catch {
    return retryForever(() => fn());
  }
}

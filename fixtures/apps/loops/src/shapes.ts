import { eq, sql } from "drizzle-orm";

import { db } from "./db";
import { counters } from "./schema";

const LIMIT = 4;

async function bump(id: number) {
  await db
    .update(counters)
    .set({ value: sql`${counters.value} + 1` })
    .where(eq(counters.id, id));
}

// BOUNDED: three iterations, each one UPDATE.
export async function countUp(id: number) {
  for (let i = 0; i < 3; i++) {
    await bump(id);
  }
}

// BOUNDED: a module constant bound, `<=` and `i += 1` (five iterations).
export async function countToLimit(id: number) {
  for (let i = 0; i <= LIMIT; i += 1) {
    await bump(id);
  }
}

// BOUNDED: `continue` skips iteration 1, `break` leaves at iteration 3:
// iterations 0 and 2 write.
export async function skipAndStop(id: number) {
  for (let i = 0; i < 5; i++) {
    if (i === 1) continue;
    if (i === 3) break;
    await bump(id);
  }
}

// BOUNDED: a `while` loop whose counter one top-level statement advances.
export async function whileCounter(id: number) {
  let n = 0;
  while (n < 2) {
    n++;
    await bump(id);
  }
}

// BOUNDED: `do...while` runs its body before the first test.
export async function doOnce(id: number) {
  let n = 5;
  do {
    await bump(id);
    n += 1;
  } while (n < 2);
}

// BOUNDED: a counting-down loop.
export async function countDown(id: number) {
  for (let i = 2; i > 0; i--) {
    await bump(id);
  }
}

// TRUNCATED: no bound from the source; the explicit attempt count ends the
// loop at run time (the third failure is re-thrown).
export async function retryForever(id: number) {
  let attempts = 0;
  while (true) {
    try {
      await bump(id);
      return attempts;
    } catch (error) {
      attempts++;
      if (attempts >= 3) throw error;
    }
  }
}

// TRUNCATED: the bound is an argument.
export async function countTo(id: number, times: number) {
  for (let i = 0; i < times; i++) {
    await bump(id);
  }
}

// TRUNCATED: the body assigns the counter other than by an increment.
export async function jumpAhead(id: number) {
  for (let i = 0; i < 3; i++) {
    await bump(id);
    if (i === 0) i = 1;
  }
}

// UNSUPPORTED: `var` in the head.
export async function varHead(id: number) {
  for (var i = 0; i < 2; i++) {
    await bump(id);
  }
}

// UNKNOWN: a condition the model does not know.
declare function more(): boolean;
export async function unknownCondition(id: number) {
  while (more()) {
    await bump(id);
  }
}

// A database error's SQLSTATE through drizzle-orm 0.45's wrapper: the
// duplicate primary key is recognized.
export async function insertOrNote(id: number) {
  try {
    await db.insert(counters).values({ id, value: 0, label: "new" });
    return "inserted";
  } catch (error) {
    if ((error as { cause?: { code?: string } }).cause?.code === "23505") {
      await db.update(counters).set({ label: "duplicate" }).where(eq(counters.id, id));
      return "duplicate";
    }
    throw error;
  }
}

// The same test written on the wrapper's own `code` (undefined with
// drizzle-orm 0.45): the duplicate is never recognized and the error
// reaches the caller.
export async function insertOrNoteOnCode(id: number) {
  try {
    await db.insert(counters).values({ id, value: 0, label: "new" });
    return "inserted";
  } catch (error) {
    if ((error as { code?: string }).code === "23505") {
      await db.update(counters).set({ label: "duplicate" }).where(eq(counters.id, id));
      return "duplicate";
    }
    throw error;
  }
}

// A thrown `Error` caught and inspected: its message is known.
export async function thrownMessage(id: number) {
  try {
    throw new Error("not today");
  } catch (error) {
    if (error instanceof Error) {
      await db.update(counters).set({ label: error.message }).where(eq(counters.id, id));
    }
  }
}

// A thrown value of an unmodeled class: its properties are unknown.
class Refusal extends Error {
  code = "REFUSED";
}
export async function thrownCustom(id: number) {
  try {
    throw new Refusal("no");
  } catch (error) {
    if ((error as { code?: string }).code === "REFUSED") {
      await bump(id);
    }
  }
}

// A route handler: the parsed body is the argument; the response is
// `{ status, body }`.
export async function POST(request: Request) {
  const body = (await request.json()) as { id: number; label: string };
  await db.update(counters).set({ label: body.label }).where(eq(counters.id, body.id));
  return Response.json({ ok: true }, { status: 201 });
}

// Reading the body twice is refused.
export async function PUT(request: Request) {
  const first = await request.json();
  const second = await request.json();
  return Response.json({ first, second });
}

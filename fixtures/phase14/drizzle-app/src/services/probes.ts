// The predicates of probe/sql-probe.mjs, one function each, so that the
// SQL the pinned Drizzle generates (reports/phase14/drizzle-sql/probe.json)
// can be compared on PostgreSQL with what the model extracts and the
// lockstep oracle renders (crates/commitrift-analyzer/tests/phase14_drizzle.rs).
import { and, eq, inArray, isNull, not, notInArray, or } from "drizzle-orm";

import { db } from "../db/client.js";
import { comments } from "../db/schema.js";

export async function inArrayWithNull() {
  await db.delete(comments).where(inArray(comments.id, [1, null, 3]));
}

export async function inArrayEmpty() {
  await db.delete(comments).where(inArray(comments.id, []));
}

export async function notInArrayNonEmpty() {
  await db.delete(comments).where(notInArray(comments.id, [1, 2]));
}

export async function notInArrayEmpty() {
  await db.delete(comments).where(notInArray(comments.id, []));
}

export async function isNullOrEq() {
  await db
    .delete(comments)
    .where(and(inArray(comments.id, [1, 2]), or(isNull(comments.status), eq(comments.status, "visible"))));
}

export async function notEq() {
  await db.delete(comments).where(not(eq(comments.status, "hidden")));
}

export async function andWithUndefined() {
  await db.delete(comments).where(and(eq(comments.id, 1), undefined));
}

export async function orWithUndefined() {
  await db.delete(comments).where(or(undefined, eq(comments.id, 1)));
}

export async function andOfNothing() {
  await db.delete(comments).where(and(undefined));
}

export async function conditionalParent(parentId: number | null) {
  await db
    .delete(comments)
    .where(
      and(
        parentId === null ? isNull(comments.parentId) : eq(comments.parentId, parentId),
        notInArray(comments.id, [5]),
      ),
    );
}

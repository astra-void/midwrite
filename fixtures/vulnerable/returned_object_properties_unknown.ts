// Returned-object properties that must not decide the caller's branch: a
// spread after the property (it may override it), a helper that sets the
// property on one path only, a binding written or exposed before the test,
// and a test inside a loop that also exposes the object.
// @expect MW1004 line=24 confidence=medium function=spreadAfter
// @expect MW1004 line=43 confidence=medium function=onePath
// @expect MW1004 line=58 confidence=medium function=writtenBefore
// @expect MW1004 line=69 confidence=medium function=exposedBefore
// @expect MW1004 line=76 confidence=medium function=testedInLoop
import { db } from "@/db";
import { identities, metadata } from "@/db/schema";

async function resolveWithOverride(link: boolean, other: { consumed?: boolean }) {
  if (link) {
    await db.insert(identities).values({ id: "1" });
    return { consumed: true, ...other };
  }
  return { consumed: false };
}

export async function spreadAfter(link: boolean, other: { consumed?: boolean }) {
  const resolved = await resolveWithOverride(link, other);
  if (resolved.consumed) return;
  await db.update(metadata).set({ seen: 1 });
}

function mark(r: { teamId: string }, quick: boolean) {
  if (quick) return { ...r, consumed: true as const };
  return { ...r };
}

async function resolveMarked(link: boolean, quick: boolean) {
  if (link) {
    await db.insert(identities).values({ id: "1" });
    return mark({ teamId: "t" }, quick);
  }
  return { teamId: "t", consumed: false };
}

export async function onePath(link: boolean, quick: boolean) {
  const resolved = await resolveMarked(link, quick);
  if (resolved.consumed) return;
  await db.update(metadata).set({ seen: 1 });
}

async function resolve(link: boolean) {
  if (link) {
    await db.insert(identities).values({ id: "1" });
    return { consumed: true };
  }
  return { consumed: false };
}

export async function writtenBefore(link: boolean) {
  const resolved = await resolve(link);
  resolved.consumed = false;
  if (resolved.consumed) return;
  await db.update(metadata).set({ seen: 1 });
}

function reset(r: { consumed: boolean }) {
  r.consumed = false;
}

export async function exposedBefore(link: boolean) {
  const resolved = await resolve(link);
  reset(resolved || { consumed: false });
  if (resolved.consumed) return;
  await db.update(metadata).set({ seen: 1 });
}

export async function testedInLoop(link: boolean, touch: (r: { consumed: boolean }) => void) {
  const resolved = await resolve(link);
  for (let i = 0; i < 2; i++) {
    if (!resolved.consumed) {
      await db.update(metadata).set({ seen: i });
    }
    touch(resolved);
  }
}

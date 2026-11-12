// Literal properties of an object argument decide the callee's branch on
// the rest object it destructures (`const { id, teamId, ...data } =
// params; if (data.status === "deleted")`). Each caller writes `events`
// first; the deletes pair with it only where the branch can run.
import { db } from "@/db";
import { events } from "@/db/schema";
import { archive, updateInbox, updateInboxAfterAudit } from "./inbox";

// Literal statuses other than "deleted": the deletes never run here.
export async function markAnalyzing(id: string, teamId: string) {
  await db.insert(events).values({ id });
  await updateInbox(db, { id, teamId, status: "analyzing" });
}

export async function markPending(id: string, teamId: string) {
  await db.insert(events).values({ id });
  await updateInbox(db, { id, teamId, status: "pending" });
}

// Status chosen by the caller: the deletes may run.
export async function setStatus(id: string, teamId: string, status: "deleted" | "pending") {
  await db.insert(events).values({ id });
  await updateInbox(db, { id, teamId, status });
}

// The literal "deleted": the deletes run.
export async function remove(id: string, teamId: string) {
  await db.insert(events).values({ id });
  await updateInbox(db, { id, teamId, status: "deleted" });
}

// No status at all: the property is not known to be absent, so the
// branch stays possible.
export async function touch(id: string, teamId: string) {
  await db.insert(events).values({ id });
  await updateInbox(db, { id, teamId });
}

// A literal through a forwarding helper is decided at the outer call.
async function forward(params: { id: string; teamId: string; status?: "deleted" | "pending" }) {
  await updateInbox(db, params);
}

export async function markPendingViaForward(id: string, teamId: string) {
  await db.insert(events).values({ id });
  await forward({ id, teamId, status: "pending" });
}

export async function removeViaForward(id: string, teamId: string) {
  await db.insert(events).values({ id });
  await forward({ id, teamId, status: "deleted" });
}

// `params` escapes before it is destructured: never decided.
export async function markAnalyzingAfterAudit(id: string, teamId: string) {
  await db.insert(events).values({ id });
  await updateInboxAfterAudit(db, { id, teamId, status: "analyzing" });
}

export async function archiveOnly(id: string) {
  await db.insert(events).values({ id });
  await archive(db, id, false);
}

export async function archiveAndPurge(id: string) {
  await db.insert(events).values({ id });
  await archive(db, id, true);
}

// The helper changes `status` before forwarding: the outer literal decides
// nothing.
async function forwardAsDeleted(params: { id: string; teamId: string; status?: "deleted" | "pending" }) {
  params.status = "deleted";
  await updateInbox(db, params);
}

export async function markPendingViaRewrite(id: string, teamId: string) {
  await db.insert(events).values({ id });
  await forwardAsDeleted({ id, teamId, status: "pending" });
}

// Decided by the literal: only the pair with the inbox update is left.
// @expect MW1004 line=12 confidence=medium function=markAnalyzing
// @expect MW1004 line=17 confidence=medium function=markPending
// @expect MW1004 line=46 confidence=medium function=markPendingViaForward
// @expect MW1004 line=62 confidence=medium function=archiveOnly
// Not decided: the pairs with the delete and with the update.
// @expect MW1004 line=23 confidence=medium function=setStatus
// @expect MW1004 line=23 confidence=medium function=setStatus
// @expect MW1004 line=36 confidence=medium function=touch
// @expect MW1004 line=36 confidence=medium function=touch
// @expect MW1004 line=57 confidence=medium function=markAnalyzingAfterAudit
// @expect MW1004 line=57 confidence=medium function=markAnalyzingAfterAudit
// @expect MW1004 line=79 confidence=medium function=markPendingViaRewrite
// @expect MW1004 line=79 confidence=medium function=markPendingViaRewrite
// The deletes always run first: only the pair with the delete.
// @expect MW1004 line=29 confidence=medium function=remove
// @expect MW1004 line=51 confidence=medium function=removeViaForward
// @expect MW1004 line=67 confidence=medium function=archiveAndPurge

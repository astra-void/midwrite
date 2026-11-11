import { and, eq } from "drizzle-orm";

import { db } from "../../../db.js";
import { devices } from "../../../schema/index.js";

// P14-E34: POST /api/devices registers a device of an account once. The
// (account_id, fingerprint) index is not unique.
export async function POST(req: Request) {
  const body = (await req.json()) as { accountId?: unknown; fingerprint?: unknown };
  if (typeof body.accountId !== "string" || typeof body.fingerprint !== "string") {
    return Response.json({ error: "invalid body" }, { status: 400 });
  }
  const { accountId, fingerprint } = body as { accountId: string; fingerprint: string };
  const [existing] = await db
    .select({ id: devices.id })
    .from(devices)
    .where(and(eq(devices.accountId, accountId), eq(devices.fingerprint, fingerprint)))
    .limit(1);
  if (existing) return Response.json({ id: existing.id }, { status: 200 });
  const [device] = await db.insert(devices).values({ accountId, fingerprint }).returning({ id: devices.id });
  return Response.json({ id: device?.id }, { status: 201 });
}

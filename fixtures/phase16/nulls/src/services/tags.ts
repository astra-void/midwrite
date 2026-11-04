import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db/client.js";
import { badges, devices, scanners, tags } from "../db/schema.js";

// Creates a tag in `groupId`, or ungrouped when it is null, unless one with
// this name is already there. Defect: for an ungrouped tag the unique key
// does not cover the duplicate (NULLS DISTINCT), so two concurrent calls
// both insert.
export async function createTag(ownerId: number, groupId: number | null, name: string) {
  const [existing] = await db
    .select({ id: tags.id })
    .from(tags)
    .where(
      and(
        eq(tags.ownerId, ownerId),
        groupId === null ? isNull(tags.groupId) : eq(tags.groupId, groupId),
        eq(tags.name, name),
      ),
    )
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(tags)
    .values({ ownerId, groupId, name })
    .returning({ id: tags.id });
  return created.id;
}

// The same for badges, whose unique key is NULLS NOT DISTINCT: the second
// concurrent insert of an ungrouped badge fails with 23505 instead of
// creating a duplicate.
export async function createBadge(ownerId: number, groupId: number | null, name: string) {
  const [existing] = await db
    .select({ id: badges.id })
    .from(badges)
    .where(
      and(
        eq(badges.ownerId, ownerId),
        groupId === null ? isNull(badges.groupId) : eq(badges.groupId, groupId),
        eq(badges.name, name),
      ),
    )
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(badges)
    .values({ ownerId, groupId, name })
    .returning({ id: badges.id });
  return created.id;
}

// Registers a device by serial number (null: not known yet), unless one
// with this serial number exists. Defect: devices without a serial number
// are never deduplicated by the key (NULLS DISTINCT).
export async function registerDevice(serialNo: string | null) {
  const [existing] = await db
    .select({ id: devices.id })
    .from(devices)
    .where(serialNo === null ? isNull(devices.serialNo) : eq(devices.serialNo, serialNo))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db.insert(devices).values({ serialNo }).returning({ id: devices.id });
  return created.id;
}

// The same for scanners (NULLS NOT DISTINCT): safe.
export async function registerScanner(serialNo: string | null) {
  const [existing] = await db
    .select({ id: scanners.id })
    .from(scanners)
    .where(serialNo === null ? isNull(scanners.serialNo) : eq(scanners.serialNo, serialNo))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db.insert(scanners).values({ serialNo }).returning({ id: scanners.id });
  return created.id;
}

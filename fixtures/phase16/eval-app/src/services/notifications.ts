import { and, eq, isNull } from "drizzle-orm";

import { db } from "../db.js";
import { muteRules, notificationRules, type NotificationEvent } from "../schema/index.js";

export type RuleResult = { status: "created" | "exists"; id: number };

// P16-E28: mutes an event on one channel, or on every channel (channelId
// null), unless the same mute rule already exists.
export async function muteEvent(
  accountId: string,
  channelId: number | null,
  event: NotificationEvent,
): Promise<RuleResult> {
  const onChannel = channelId === null ? isNull(muteRules.channelId) : eq(muteRules.channelId, channelId);
  const sameRule = and(eq(muteRules.accountId, accountId), onChannel, eq(muteRules.event, event));

  const [existing] = await db.select({ id: muteRules.id }).from(muteRules).where(sameRule).limit(1);
  if (existing) return { status: "exists", id: existing.id };

  const [created] = await db
    .insert(muteRules)
    .values({ accountId, channelId, event })
    .onConflictDoNothing()
    .returning({ id: muteRules.id });
  if (created) return { status: "created", id: created.id };

  const [winner] = await db.select({ id: muteRules.id }).from(muteRules).where(sameRule).limit(1);
  if (!winner) throw new Error("mute rule vanished");
  return { status: "exists", id: winner.id };
}

// P16-E29: the same for notification rules (UNIQUE ... NULLS NOT DISTINCT).
export async function addNotificationRule(
  accountId: string,
  channelId: number | null,
  event: NotificationEvent,
): Promise<RuleResult> {
  const onChannel =
    channelId === null ? isNull(notificationRules.channelId) : eq(notificationRules.channelId, channelId);
  const sameRule = and(eq(notificationRules.accountId, accountId), onChannel, eq(notificationRules.event, event));

  const [existing] = await db
    .select({ id: notificationRules.id })
    .from(notificationRules)
    .where(sameRule)
    .limit(1);
  if (existing) return { status: "exists", id: existing.id };

  const [created] = await db
    .insert(notificationRules)
    .values({ accountId, channelId, event })
    .onConflictDoNothing()
    .returning({ id: notificationRules.id });
  if (created) return { status: "created", id: created.id };

  const [winner] = await db
    .select({ id: notificationRules.id })
    .from(notificationRules)
    .where(sameRule)
    .limit(1);
  if (!winner) throw new Error("notification rule vanished");
  return { status: "exists", id: winner.id };
}

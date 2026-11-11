import { index, integer, pgTable, serial, text, unique, uuid } from "drizzle-orm/pg-core";

import { accounts } from "./accounts.js";

export type NotificationEvent = "comment" | "mention" | "assignment" | "digest";

export const events = pgTable("events", {
  id: serial("id").primaryKey(),
  title: text("title").notNull(),
  capacity: integer("capacity").notNull(),
});

// Waitlist: (event_id, email) indexed, NOT unique.
export const waitlistEntries = pgTable(
  "waitlist_entries",
  {
    id: serial("id").primaryKey(),
    eventId: integer("event_id")
      .notNull()
      .references(() => events.id),
    email: text("email").notNull(),
    name: text("name"),
  },
  (t) => [index("waitlist_entries_event_email").on(t.eventId, t.email)],
);

// RSVPs: UNIQUE(event_id, email).
export const rsvps = pgTable(
  "rsvps",
  {
    id: serial("id").primaryKey(),
    eventId: integer("event_id")
      .notNull()
      .references(() => events.id),
    email: text("email").notNull(),
    name: text("name"),
  },
  (t) => [unique("rsvps_event_email").on(t.eventId, t.email)],
);

export const notificationChannels = pgTable("notification_channels", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  kind: text("kind").notNull(),
});

// Mute rules: channel_id NULL means "every channel".
// UNIQUE(account_id, channel_id, event) with the default NULLS DISTINCT.
export const muteRules = pgTable(
  "mute_rules",
  {
    id: serial("id").primaryKey(),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
    channelId: integer("channel_id").references(() => notificationChannels.id),
    event: text("event").$type<NotificationEvent>().notNull(),
  },
  (t) => [unique("mute_rules_account_channel_event").on(t.accountId, t.channelId, t.event)],
);

// Notification rules: the same shape, with UNIQUE ... NULLS NOT DISTINCT
// (PostgreSQL 15+): two "every channel" rules for the same event conflict.
export const notificationRules = pgTable(
  "notification_rules",
  {
    id: serial("id").primaryKey(),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
    channelId: integer("channel_id").references(() => notificationChannels.id),
    event: text("event").$type<NotificationEvent>().notNull(),
  },
  (t) => [
    unique("notification_rules_account_channel_event").on(t.accountId, t.channelId, t.event).nullsNotDistinct(),
  ],
);

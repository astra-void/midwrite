import { sql } from "drizzle-orm";
import {
  boolean,
  index,
  integer,
  jsonb,
  pgTable,
  serial,
  text,
  timestamp,
  unique,
  uniqueIndex,
  uuid,
} from "drizzle-orm/pg-core";

// Account-level settings, stored as one jsonb document.
export type AccountSettings = {
  plan: "free" | "pro";
  theme?: "light" | "dark";
  locale?: string;
  weeklyDigest?: boolean;
};

export const accounts = pgTable("accounts", {
  id: uuid("id").defaultRandom().primaryKey(),
  email: text("email").notNull().unique(),
  displayName: text("display_name").notNull(),
  timezone: text("timezone").notNull().default("UTC"),
  settings: jsonb("settings").$type<AccountSettings>().notNull().default({ plan: "free" }),
  trialUsed: boolean("trial_used").notNull().default(false),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  updatedAt: timestamp("updated_at", { withTimezone: true })
    .notNull()
    .defaultNow()
    .$onUpdate(() => new Date()),
});

// Metered usage: one row per account.
export const usage = pgTable("usage", {
  accountId: uuid("account_id")
    .primaryKey()
    .references(() => accounts.id),
  used: integer("used").notNull().default(0),
  updatedAt: timestamp("updated_at", { withTimezone: true })
    .notNull()
    .defaultNow()
    .$onUpdate(() => new Date()),
});

// Credit grants. grant_key is an optional idempotency key; the unique
// constraint is NULLS DISTINCT, so rows without a key never conflict.
export const credits = pgTable("credits", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  reason: text("reason").notNull(),
  amount: integer("amount").notNull(),
  grantKey: text("grant_key").unique("credits_grant_key"),
  grantedAt: timestamp("granted_at", { withTimezone: true }).notNull().defaultNow(),
});

export const trials = pgTable("trials", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  plan: text("plan").notNull(),
  startedAt: timestamp("started_at", { withTimezone: true }).notNull().defaultNow(),
});

// At most one active subscription per account: a partial unique index.
export const subscriptions = pgTable(
  "subscriptions",
  {
    id: serial("id").primaryKey(),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
    plan: text("plan").notNull(),
    status: text("status").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  },
  (t) => [uniqueIndex("subscriptions_one_active").on(t.accountId).where(sql`${t.status} = 'active'`)],
);

// Devices are looked up by (account, fingerprint); the index is not unique.
export const devices = pgTable(
  "devices",
  {
    id: serial("id").primaryKey(),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
    fingerprint: text("fingerprint").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  },
  (t) => [index("devices_account_fingerprint").on(t.accountId, t.fingerprint)],
);

export const apiClients = pgTable(
  "api_clients",
  {
    id: serial("id").primaryKey(),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
    name: text("name").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  },
  (t) => [unique("api_clients_account_name").on(t.accountId, t.name)],
);

export const onboardingCalls = pgTable("onboarding_calls", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  requestedAt: timestamp("requested_at", { withTimezone: true }).notNull().defaultNow(),
});

import { boolean, integer, pgTable, serial, text, timestamp, uuid } from "drizzle-orm/pg-core";

export type Theme = "light" | "dark" | "contrast";

export const accounts = pgTable("accounts", {
  id: uuid("id").defaultRandom().primaryKey(),
  email: text("email").notNull().unique(),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
});

// One profile per account. theme_changes counts the times the theme was
// actually changed (a request for the current theme is not a change).
export const profiles = pgTable("profiles", {
  accountId: uuid("account_id")
    .primaryKey()
    .references(() => accounts.id),
  nickname: text("nickname").notNull(),
  theme: text("theme").$type<Theme>().notNull().default("light"),
  themeChanges: integer("theme_changes").notNull().default(0),
});

export const notes = pgTable("notes", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  body: text("body").notNull(),
  archived: boolean("archived").notNull().default(false),
});

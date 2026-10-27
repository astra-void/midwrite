import { integer, pgTable, serial, text } from "drizzle-orm/pg-core";

import { calls, nextStamp } from "../../lib/calls.js";

// Application UUID.
export const notes = pgTable("notes", {
  id: text("id")
    .$defaultFn(() => crypto.randomUUID())
    .primaryKey(),
  body: text("body").notNull(),
  // An application default the model does not recognize (side effect).
  stamp: text("stamp").$defaultFn(() => nextStamp()),
});

// Sequence, identity and constant defaults.
export const counters = pgTable("counters", {
  id: serial("id").primaryKey(),
  label: text("label").notNull(),
  state: text("state").notNull().default("new"),
});

export const tickets = pgTable("tickets", {
  id: integer("id").primaryKey().generatedByDefaultAsIdentity(),
  title: text("title").notNull(),
});

export { calls };

import { pgTable, text, uuid } from "drizzle-orm/pg-core";

// Database UUID.
export const people = pgTable("people", {
  id: uuid("id").defaultRandom().primaryKey(),
  email: text("email").notNull().unique(),
});

export const profiles = pgTable("profiles", {
  id: uuid("id").defaultRandom().primaryKey(),
  personId: uuid("person_id")
    .notNull()
    .references(() => people.id),
  bio: text("bio").notNull(),
});

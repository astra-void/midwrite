import { integer, pgTable, serial, text, unique } from "drizzle-orm/pg-core";

// Tags of an owner, optionally inside a group (group_id IS NULL: ungrouped).
// UNIQUE(owner_id, group_id, name) with PostgreSQL's default NULLS DISTINCT:
// two ungrouped tags with one name do not conflict.
export const tags = pgTable(
  "tags",
  {
    id: serial("id").primaryKey(),
    ownerId: integer("owner_id").notNull(),
    groupId: integer("group_id"),
    name: text("name").notNull(),
  },
  (t) => [unique("tags_owner_group_name").on(t.ownerId, t.groupId, t.name)],
);

// The same for badges, with NULLS NOT DISTINCT (PostgreSQL 15+): a NULL
// group equals a NULL group, so one ungrouped badge per name.
export const badges = pgTable(
  "badges",
  {
    id: serial("id").primaryKey(),
    ownerId: integer("owner_id").notNull(),
    groupId: integer("group_id"),
    name: text("name").notNull(),
  },
  (t) => [unique("badges_owner_group_name").on(t.ownerId, t.groupId, t.name).nullsNotDistinct()],
);

// Single nullable column, column-level spelling. Default NULLS DISTINCT:
// any number of devices without a serial number.
export const devices = pgTable("devices", {
  id: serial("id").primaryKey(),
  serialNo: text("serial_no").unique("devices_serial_no"),
});

// The same with `{ nulls: "not distinct" }`: at most one device without a
// serial number.
export const scanners = pgTable("scanners", {
  id: serial("id").primaryKey(),
  serialNo: text("serial_no").unique("scanners_serial_no", { nulls: "not distinct" }),
});

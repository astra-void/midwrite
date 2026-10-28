// Phase 14 Track D: the SQL and parameters the pinned drizzle-orm (0.45.4)
// generates for the constructs the model gained, without a database
// (`drizzle.mock()` + `.toSQL()`). The tables mirror `src/db/schema.ts`.
// Clock readings (`new Date()`) are replaced by "<clock>" so the output is
// stable. Usage: node probe/sql-probe.mjs > ../../../reports/phase14/drizzle-sql/probe.json
import { and, eq, inArray, isNull, not, notInArray, or, sql } from "drizzle-orm";
import { drizzle } from "drizzle-orm/node-postgres";
import { integer, jsonb, pgTable, serial, text, timestamp, unique } from "drizzle-orm/pg-core";
import { readFileSync } from "node:fs";

const version = JSON.parse(
  readFileSync(new URL("../node_modules/drizzle-orm/package.json", import.meta.url), "utf8"),
).version;

const orders = pgTable("orders", {
  id: integer("id").primaryKey(),
  customer: text("customer").notNull(),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
});
const orderItems = pgTable("order_items", {
  id: serial("id").primaryKey(),
  orderId: integer("order_id").notNull().references(() => orders.id),
  sku: text("sku").notNull(),
  qty: integer("qty").notNull(),
});
const postTags = pgTable(
  "post_tags",
  { id: serial("id").primaryKey(), postId: integer("post_id").notNull(), tag: text("tag").notNull() },
  (t) => [unique("post_tags_post_tag").on(t.postId, t.tag)],
);
const comments = pgTable("comments", {
  id: integer("id").primaryKey(),
  parentId: integer("parent_id"),
  body: text("body").notNull(),
  status: text("status"),
});
const users = pgTable("users", {
  id: integer("id").primaryKey(),
  email: text("email").notNull(),
  welcomedAt: timestamp("welcomed_at", { withTimezone: true }),
  updatedAt: timestamp("updated_at", { withTimezone: true }).$onUpdate(() => new Date()),
});
const counters = pgTable("counters", {
  name: text("name").primaryKey(),
  hits: integer("hits").notNull(),
  updatedAt: timestamp("updated_at", { withTimezone: true }),
});
const settings = pgTable("settings", { userId: integer("user_id").primaryKey(), prefs: jsonb("prefs") });

const db = drizzle.mock();
const iso = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;
const stable = (p) => (typeof p === "string" && iso.test(p) ? "<clock>" : p);

function probe(name, builder) {
  try {
    const { sql: text, params } = builder().toSQL();
    return { name, sql: text, params: params.map(stable) };
  } catch (e) {
    return { name, error: String(e && e.message ? e.message : e) };
  }
}

const parentId = null;
const out = [
  probe("multi-row insert, serial key omitted", () =>
    db.insert(orderItems).values([
      { orderId: 1, sku: "a", qty: 1 },
      { orderId: 1, sku: "b", qty: 2 },
    ])),
  probe("multi-row insert, rows with different columns", () =>
    db.insert(orderItems).values([
      { id: 10, orderId: 1, sku: "a", qty: 1 },
      { orderId: 1, sku: "b", qty: 2 },
    ])),
  probe("multi-row insert on conflict do nothing returning a column", () =>
    db.insert(postTags)
      .values([{ postId: 1, tag: "x" }, { postId: 1, tag: "y" }])
      .onConflictDoNothing()
      .returning({ tag: postTags.tag })),
  probe("multi-row upsert with new Date() in set", () =>
    db.insert(counters)
      .values([{ name: "a", hits: 1 }, { name: "b", hits: 1 }])
      .onConflictDoUpdate({ target: counters.name, set: { hits: 7, updatedAt: new Date() } })),
  probe("values([]) (empty)", () => db.insert(orderItems).values([])),
  probe("inArray, non-empty with a null", () =>
    db.select().from(comments).where(inArray(comments.id, [1, null, 3]))),
  probe("inArray, empty", () => db.select().from(comments).where(inArray(comments.id, []))),
  probe("notInArray, non-empty", () => db.select().from(comments).where(notInArray(comments.id, [1, 2]))),
  probe("notInArray, empty", () => db.select().from(comments).where(notInArray(comments.id, []))),
  probe("isNull / or / eq", () =>
    db.update(comments)
      .set({ status: "hidden" })
      .where(and(inArray(comments.id, [1, 2]), or(isNull(comments.status), eq(comments.status, "visible"))))),
  probe("not(eq)", () => db.select().from(comments).where(not(eq(comments.status, "hidden")))),
  probe("and with an undefined operand", () =>
    db.select().from(comments).where(and(eq(comments.id, 1), undefined))),
  probe("or with an undefined operand", () =>
    db.select().from(comments).where(or(undefined, eq(comments.id, 1)))),
  probe("and() of nothing", () => db.select().from(comments).where(and(undefined))),
  probe("conditional predicate (parentId === null)", () =>
    db.delete(comments).where(
      and(
        parentId === null ? isNull(comments.parentId) : eq(comments.parentId, parentId),
        notInArray(comments.id, [5]),
      ),
    )),
  probe("insert omitting a defaultNow() column", () => db.insert(orders).values({ id: 1, customer: "c" })),
  probe("insert omitting a $onUpdate column (no default)", () => db.insert(users).values({ id: 1, email: "e" })),
  probe("update without the $onUpdate column", () =>
    db.update(users).set({ welcomedAt: new Date() }).where(and(eq(users.id, 1), isNull(users.welcomedAt)))),
  probe("set with sql`now()`", () => db.update(counters).set({ updatedAt: sql`now()` }).where(eq(counters.name, "a"))),
  probe("jsonb insert", () => db.insert(settings).values({ userId: 1, prefs: { theme: "dark", beta: true } })),
  probe("jsonb eq", () => db.select().from(settings).where(eq(settings.prefs, { theme: "dark" }))),
  probe("upsert of a table with a $onUpdate column", () =>
    db.insert(users)
      .values({ id: 1, email: "e" })
      .onConflictDoUpdate({ target: users.id, set: { email: "f" } })),
  probe("conditional predicate (parentId === 4)", () => {
    const parentId = 4;
    return db.delete(comments).where(
      and(
        parentId === null ? isNull(comments.parentId) : eq(comments.parentId, parentId),
        notInArray(comments.id, [5]),
      ),
    );
  }),
];

console.log(JSON.stringify({ drizzleOrm: version, probes: out }, null, 2));

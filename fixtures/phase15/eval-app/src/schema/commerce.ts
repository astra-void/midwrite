import { index, integer, pgTable, serial, uuid } from "drizzle-orm/pg-core";

import { accounts } from "./accounts.js";

// A cart is created on the first add. carts.account_id is indexed, not unique.
export const carts = pgTable(
  "carts",
  {
    id: serial("id").primaryKey(),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
  },
  (t) => [index("carts_account").on(t.accountId)],
);

export const cartItems = pgTable("cart_items", {
  id: serial("id").primaryKey(),
  cartId: integer("cart_id")
    .notNull()
    .references(() => carts.id),
  productId: integer("product_id").notNull(),
  quantity: integer("quantity").notNull(),
});

// One wishlist per account: wishlists.account_id is UNIQUE.
export const wishlists = pgTable("wishlists", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .unique("wishlists_account")
    .references(() => accounts.id),
});

export const wishlistItems = pgTable("wishlist_items", {
  id: serial("id").primaryKey(),
  wishlistId: integer("wishlist_id")
    .notNull()
    .references(() => wishlists.id),
  productId: integer("product_id").notNull(),
});

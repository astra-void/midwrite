import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { cartItems, carts, wishlistItems, wishlists } from "../schema/index.js";

// P15-E16: adds a product to the account's cart, creating the cart on the
// first add.
export async function addToCart(accountId: string, productId: number, quantity: number) {
  const [cart] = await db.select({ id: carts.id }).from(carts).where(eq(carts.accountId, accountId)).limit(1);
  if (cart) {
    await db.insert(cartItems).values({ cartId: cart.id, productId, quantity });
    return cart.id;
  }
  const [created] = await db.insert(carts).values({ accountId }).returning({ id: carts.id });
  if (!created) throw new Error("cart insert failed");
  await db.insert(cartItems).values({ cartId: created.id, productId, quantity });
  return created.id;
}

// P15-E17: adds a product to the account's wishlist, creating the wishlist on
// the first add (wishlists.account_id is unique).
export async function addToWishlist(accountId: string, productId: number) {
  await db.insert(wishlists).values({ accountId }).onConflictDoNothing({ target: wishlists.accountId });
  const [wishlist] = await db
    .select({ id: wishlists.id })
    .from(wishlists)
    .where(eq(wishlists.accountId, accountId))
    .limit(1);
  if (!wishlist) throw new Error("wishlist missing");
  await db.insert(wishlistItems).values({ wishlistId: wishlist.id, productId });
  return wishlist.id;
}

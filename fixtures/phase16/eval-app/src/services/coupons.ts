import { eq } from "drizzle-orm";

import { db, replicatedDb } from "../db.js";
import { couponRedemptions, coupons } from "../schema/index.js";

export type Redemption =
  | { status: "redeemed"; percentOff: number }
  | { status: "already-redeemed" }
  | { status: "unknown-code" };

// P16-E36: redeems a single-use coupon. The coupon is looked up through the
// replica set; the redemption is written on the primary.
export async function redeemCoupon(code: string, accountId: string): Promise<Redemption> {
  const [coupon] = await replicatedDb
    .select({ id: coupons.id, redeemed: coupons.redeemed, percentOff: coupons.percentOff })
    .from(coupons)
    .where(eq(coupons.code, code))
    .limit(1);
  if (!coupon) return { status: "unknown-code" };
  if (coupon.redeemed) return { status: "already-redeemed" };

  await db.transaction(async (tx) => {
    await tx.update(coupons).set({ redeemed: true }).where(eq(coupons.id, coupon.id));
    await tx.insert(couponRedemptions).values({ couponId: coupon.id, accountId });
  });
  return { status: "redeemed", percentOff: coupon.percentOff };
}

// P16-E37: the same, with the lookup in the redeeming transaction (on the
// primary) under a row lock.
export async function redeemCouponLocked(code: string, accountId: string): Promise<Redemption> {
  return replicatedDb.transaction(async (tx): Promise<Redemption> => {
    const [coupon] = await tx
      .select({ id: coupons.id, redeemed: coupons.redeemed, percentOff: coupons.percentOff })
      .from(coupons)
      .where(eq(coupons.code, code))
      .for("update");
    if (!coupon) return { status: "unknown-code" };
    if (coupon.redeemed) return { status: "already-redeemed" };

    await tx.update(coupons).set({ redeemed: true }).where(eq(coupons.id, coupon.id));
    await tx.insert(couponRedemptions).values({ couponId: coupon.id, accountId });
    return { status: "redeemed", percentOff: coupon.percentOff };
  });
}

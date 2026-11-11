import { randomUUID } from "node:crypto";

import { nanoid } from "nanoid";

// Payment intent ids look like the payment provider's: "pi_" + 24 random
// URL-safe characters.
export function newPaymentIntentId(): string {
  return `pi_${nanoid(24)}`;
}

// Refresh tokens are opaque random UUIDs.
export function newRefreshToken(): string {
  return randomUUID();
}

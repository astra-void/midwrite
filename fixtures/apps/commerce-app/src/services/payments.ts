// The payment-provider seam. The process entry point configures the SDK
// client; nothing in the checkout code knows which client it is.
export interface ChargeRequest {
  orderId: number;
  amount: number;
}

export interface PaymentProvider {
  charge(request: ChargeRequest, idempotencyKey?: string): Promise<unknown>;
  payout(request: ChargeRequest): Promise<unknown>;
}

let configured: PaymentProvider | undefined;

export function usePaymentProvider(provider: PaymentProvider): void {
  if (configured) throw new Error("the payment provider is already configured");
  configured = provider;
}

function provider(): PaymentProvider {
  if (!configured) throw new Error("no payment provider: call usePaymentProvider() at startup");
  return configured;
}

/** Charge the customer's card; the key is sent to the provider when given. */
export async function chargeCard(request: ChargeRequest, idempotencyKey?: string): Promise<void> {
  await provider().charge(request, idempotencyKey);
}

/** Send money back to the customer. The provider does not deduplicate payouts. */
export async function sendPayout(request: ChargeRequest): Promise<void> {
  await provider().payout(request);
}

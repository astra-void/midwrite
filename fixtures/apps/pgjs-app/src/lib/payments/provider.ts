// The application's payment-provider seam. In production the process entry
// point configures the provider SDK client; nothing in the checkout code
// knows which client it is.
export interface ChargeRequest {
  orderId: number;
  amount: number;
}

export interface PaymentProvider {
  charge(request: ChargeRequest, idempotencyKey?: string): Promise<unknown>;
}

let configured: PaymentProvider | undefined;

export function usePaymentProvider(provider: PaymentProvider): void {
  if (configured) throw new Error("the payment provider is already configured");
  configured = provider;
}

/**
 * Charge the customer's card. `idempotencyKey`, when given, is sent to the
 * provider; whether the provider deduplicates by it is the provider's
 * contract, not something this code can ensure.
 */
export async function chargeCard(request: ChargeRequest, idempotencyKey?: string): Promise<void> {
  if (!configured) throw new Error("no payment provider: call usePaymentProvider() at startup");
  await configured.charge(request, idempotencyKey);
}

// The application's confirmation seam (an e-mail or an event for the order
// service). The process entry point configures the client.
export interface Confirmation {
  orderId: number;
  requestId: string;
}

export interface ConfirmationClient {
  publish(confirmation: Confirmation): Promise<unknown>;
}

let configured: ConfirmationClient | undefined;

export function useConfirmations(client: ConfirmationClient): void {
  if (configured) throw new Error("confirmations are already configured");
  configured = client;
}

/** Tell the customer (and the order service) that the payment was recorded. */
export async function publishConfirmation(confirmation: Confirmation): Promise<void> {
  if (!configured) throw new Error("no confirmation client: call useConfirmations() at startup");
  await configured.publish(confirmation);
}

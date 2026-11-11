// The application's messaging seams: the event bus (order events for other
// services) and the mailer. The process entry point configures the clients.
export interface OrderEvent {
  topic: string;
  orderId: number;
}

export interface Receipt {
  orderId: number;
}

export interface StockNotice {
  productId: number;
  quantity: number;
}

export interface Messaging {
  publish(event: OrderEvent, messageId?: number): Promise<unknown>;
  sendEmail(receipt: Receipt): Promise<unknown>;
  notifyWarehouse(notice: StockNotice): Promise<unknown>;
}

let configured: Messaging | undefined;

export function useMessaging(messaging: Messaging): void {
  if (configured) throw new Error("messaging is already configured");
  configured = messaging;
}

function messaging(): Messaging {
  if (!configured) throw new Error("no messaging client: call useMessaging() at startup");
  return configured;
}

/** Publish an order event. `messageId` lets consumers detect redeliveries. */
export async function publishEvent(event: OrderEvent, messageId?: number): Promise<void> {
  await messaging().publish(event, messageId);
}

/** Send the customer a receipt e-mail. The mailer does not deduplicate. */
export async function sendReceiptEmail(receipt: Receipt): Promise<void> {
  await messaging().sendEmail(receipt);
}

/** Tell the warehouse to set stock aside. The warehouse does not deduplicate. */
export async function notifyWarehouse(notice: StockNotice): Promise<void> {
  await messaging().notifyWarehouse(notice);
}

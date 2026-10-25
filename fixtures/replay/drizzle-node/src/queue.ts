// The application's message queue client. The transport (a broker client in
// production, a local event log under replay) is configured by the process
// entry point. Publishing is an external effect: it is not part of any
// database transaction.

export type Transport = (topic: string, payload: unknown, options: { id?: string }) => Promise<void>;

let transport: Transport | undefined;

export function useTransport(next: Transport): void {
  if (transport) throw new Error("the message transport is already configured");
  transport = next;
}

export const queue = {
  async publish(topic: string, payload: unknown, options: { id?: string } = {}): Promise<void> {
    if (!transport) throw new Error("no message transport configured");
    await transport(topic, payload, options);
  },
};

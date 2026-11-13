// A package's SDK client held in a field (`private client = new Resend()`,
// set once in the constructor) is that client, and a private method
// (`this.#createInternal()`) is followed like any other: the email that
// `Notifications.create` sends through `EmailService` goes out before the
// invoice update commits.
// @expect MW1002 line=38 confidence=medium function=sendInvoice
import { Resend } from "resend";
import { db } from "@/db";
import { invoices } from "@/db/schema";

class EmailService {
  private client: Resend;
  constructor() {
    this.client = new Resend(process.env.RESEND_API_KEY!);
  }
  async sendBulk(emails: { to: string }[]) {
    for (const email of emails) {
      await this.client.emails.send({ from: "a@b.c", to: email.to, subject: "x", html: "y" });
    }
  }
}

export class Notifications {
  #emailService: EmailService;
  constructor() {
    this.#emailService = new EmailService();
  }
  async create(to: string) {
    return this.#createInternal(to);
  }
  async #createInternal(to: string) {
    await this.#emailService.sendBulk([{ to }]);
  }
}

export async function sendInvoice(id: string, to: string) {
  const notifications = new Notifications();
  await notifications.create(to);
  await db.update(invoices).set({ status: "unpaid" });
}

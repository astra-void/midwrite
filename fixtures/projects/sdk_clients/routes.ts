// DEV-148: SDK clients imported from project modules are recognized by
// provenance (`builtinEffects`), not by name.
import { drizzle } from "drizzle-orm/node-postgres";
import { resend } from "./services/resend";
import { createAdminClient } from "./services/supabase";
import { getQueue, mailClient } from "./services";
import { resend as lookalike } from "./lookalike";

const store = drizzle(process.env.DATABASE_URL!);

export async function removeContact(email: string) {
  await store.update(users).set({ subscribed: false });
  await resend.contacts.remove({ email });
}

export async function deleteAccount(id: string) {
  const admin = await createAdminClient();
  await store.delete(users);
  await admin.auth.admin.deleteUser(id);
}

export async function sendViaBarrel(to: string) {
  await store.insert(outbox).values({ to });
  await mailClient.emails.send({ to });
}

export async function cancelSchedule(id: string, jobId: string) {
  const queue = getQueue("invoices");
  const job = await queue.getJob(jobId);
  if (job) {
    await job.remove();
  }
  await store.update(invoices).set({ status: "draft" });
}

export async function notLookalike(to: string) {
  await store.insert(outbox).values({ to });
  await lookalike.emails.send({ to });
}
// @expect MW1001 line=13 function=removeContact
// @expect MW1001 line=19 function=deleteAccount
// @expect MW1001 line=24 function=sendViaBarrel
// @expect MW1002 line=31 function=cancelSchedule
// @expect MW9001 line=38 function=notLookalike

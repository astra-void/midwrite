import { db } from "../db.js";
import { exportJobs } from "../schema/index.js";

// P14-E06: queues an export of the account's data. The job key is built from
// the account and the normalized format and is the primary key of
// export_jobs.
export async function queueExport(accountId: string, format: string) {
  const ext = format.trim().toLowerCase();
  if (ext !== "csv" && ext !== "json") throw new Error(`unsupported export format: ${format}`);
  const jobKey = "export:" + accountId + ":" + ext;
  const queued = await db
    .insert(exportJobs)
    .values({ jobKey, accountId, format: ext })
    .onConflictDoNothing({ target: exportJobs.jobKey })
    .returning({ jobKey: exportJobs.jobKey });
  return { queued: queued.length > 0, jobKey };
}

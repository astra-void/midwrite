// Stand-in for midday's job client (packages/job-client): enqueues a job.
// Here the "queue" is the replay event log; the job name is the topic.
import { queue } from "../../shared/queue.ts";

export async function triggerJob(jobName: string, payload: unknown, _queueName: string) {
  await queue.publish(jobName, payload);
}

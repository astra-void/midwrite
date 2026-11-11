import { Queue } from "bullmq";

const queues = new Map<string, Queue>();

export function getQueue(name: string): Queue {
  let queue = queues.get(name);
  if (!queue) {
    queue = new Queue(name);
    queues.set(name, queue);
  }
  return queue;
}
// @expect none

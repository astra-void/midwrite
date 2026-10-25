// A fake message queue backed by a durable, append-only event log file that
// the replay supervisor reads after the run. It stands for an external
// system: it is NOT part of the database, so a database transaction cannot
// make a publication atomic with a write.
//
// A publication is acknowledged (`after-event-publish`) only after the line
// is written and fsync'ed. Consumers deduplicate by `id` when one is given.

import { closeSync, fsyncSync, openSync, writeSync } from "node:fs";
import { checkpoint, eventLogPath } from "../../../runtime/typescript/commitrift-replay.ts";

export const queue = {
  async publish(topic: string, payload: unknown, options?: { id?: string }): Promise<void> {
    await checkpoint("before-event-publish", topic);
    const fd = openSync(eventLogPath(), "a");
    try {
      writeSync(fd, JSON.stringify({ topic, payload, id: options?.id }) + "\n");
      fsyncSync(fd);
    } finally {
      closeSync(fd);
    }
    await checkpoint("after-event-publish", topic);
  },
};

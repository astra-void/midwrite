// A fake message transport for replay harnesses: an append-only event log
// file inside the replay root that the supervisor reads after the run (the
// manifest's `events` observations). It stands for an external system: it
// is not part of the database, so a database transaction cannot make a
// publication atomic with a write.
//
// A publication is acknowledged (`after-event-publish`) only after the line
// is written and fsync'ed.

import { closeSync, fsyncSync, openSync, writeSync } from "node:fs";
import { checkpoint, eventLogPath } from "./index.js";

/**
 * Append `{ topic, payload, id }` to the run's event log, with the
 * checkpoints `before-event-publish` and `after-event-publish` (label:
 * topic) around the durable append.
 */
export async function publishToEventLog(topic, payload, options = {}) {
  await checkpoint("before-event-publish", topic);
  const fd = openSync(eventLogPath(), "a");
  try {
    writeSync(fd, JSON.stringify({ topic, payload, id: options.id }) + "\n");
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
  await checkpoint("after-event-publish", topic);
}

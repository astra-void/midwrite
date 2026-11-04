/**
 * Append `{ topic, payload, id }` to the run's event log (fsync'ed), with
 * the checkpoints `before-event-publish` and `after-event-publish` (label:
 * topic) around the append.
 */
export declare function publishToEventLog(
  topic: string,
  payload: unknown,
  options?: { id?: string },
): Promise<void>;

// Counts how often the application-side default callbacks run, so the
// probe can observe when Drizzle evaluates them.
export const calls = { noteId: 0, stamp: 0 };

export function nextStamp(): string {
  calls.stamp += 1;
  return `stamp-${calls.stamp}`;
}

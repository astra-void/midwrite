// Runs the operation and, if it throws for any reason (a lost connection, a
// timeout waiting for the COMMIT acknowledgment, ...), runs it once more.
export async function withRetry<T>(operation: () => Promise<T>): Promise<T> {
  try {
    return await operation();
  } catch {
    return await operation();
  }
}

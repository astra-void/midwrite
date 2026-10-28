// Error codes after which the operation may or may not have taken effect and
// is worth running again: a dropped or reset connection, a timeout waiting
// for the server's reply, a server shutdown (57P01) or a lost connection
// reported by the server (08006).
const RETRYABLE = ["ECONNRESET", "ETIMEDOUT", "EPIPE", "57P01", "08006"];

// Runs the operation; if it fails with a retryable error, runs it again, up to
// `attempts` runs in total. Any other error is thrown at once.
export async function withRetry<T>(operation: () => Promise<T>, attempts = 3): Promise<T> {
  let lastError: unknown;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      return await operation();
    } catch (error) {
      lastError = error;
      const code = (error as { code?: unknown }).code;
      if (typeof code !== "string" || !RETRYABLE.includes(code)) throw error;
    }
  }
  throw lastError;
}

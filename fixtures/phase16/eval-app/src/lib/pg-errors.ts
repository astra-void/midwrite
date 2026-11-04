// Helpers for PostgreSQL / node-postgres errors.

// Failures after which a statement or a COMMIT may or may not have taken
// effect on the server: a reset or timed-out connection, an administrator
// shutdown (57P01), a connection failure (08006) or a connection that no
// longer exists (08003).
const CONNECTION_ERRORS = new Set(["ECONNRESET", "ETIMEDOUT", "EPIPE", "57P01", "08006", "08003"]);

export function errorCode(error: unknown): string | undefined {
  if (typeof error !== "object" || error === null) return undefined;
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" ? code : undefined;
}

// SQLSTATE 40001: the transaction was rolled back because it could not be
// serialized; running it again from the start is the documented remedy.
export function isSerializationFailure(error: unknown): boolean {
  return errorCode(error) === "40001";
}

export function isConnectionError(error: unknown): boolean {
  const code = errorCode(error);
  return code !== undefined && CONNECTION_ERRORS.has(code);
}

export function backoffMs(attempt: number): number {
  return Math.min(1_000, 25 * 2 ** attempt);
}

export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export class InsufficientFundsError extends Error {
  constructor(readonly accountId: number) {
    super(`insufficient funds on ${accountId}`);
  }
}

export function assertPositiveAmount(amount: number): void {
  if (!Number.isSafeInteger(amount) || amount <= 0) throw new RangeError("amount must be a positive integer");
}

// SQLSTATE 23503: an INSERT or UPDATE referenced a missing parent row, or a
// DELETE left referencing rows behind.
export function isForeignKeyViolation(error: unknown): boolean {
  return typeof error === "object" && error !== null && (error as { code?: unknown }).code === "23503";
}

// @expect none
export async function refreshMetadata() {
  // A method-filtered rule does not assert that a GET mutates remote state.
  await fetch("https://example.test/metadata", { method: "GET" });
  await db.update(connections).set({ refreshed: true });
}

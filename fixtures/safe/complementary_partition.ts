// Growing and shrinking an unchanged complementary partition are exclusive.
// @expect none
export async function resize(rows: { enabled: boolean }[], requested: number) {
  const ordered = [
    ...rows.filter((row) => row.enabled),
    ...rows.filter((row) => !row.enabled),
  ];
  const removed = ordered.slice(requested);
  if (requested > rows.length) {
    await db.insert(items).values({});
  }
  if (removed.length > 0) {
    await db.delete(items);
  }
}

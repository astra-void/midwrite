// @expect none
// A module-private helper whose only caller passes the transaction: its two
// inserts are atomic with the caller's (no `closedWorld` needed).
async function createCategories(db: Database, teamId: string) {
  const parents = await db.insert(categories).values([{ teamId }]).returning({ id: categories.id });
  await db.insert(categories).values(parents.map((p) => ({ teamId, parentId: p.id })));
}
export async function createTeam(db: Database, name: string) {
  return db.transaction(async (tx) => {
    const [team] = await tx.insert(teams).values({ name }).returning({ id: teams.id });
    await createCategories(tx, team.id);
    return team.id;
  });
}

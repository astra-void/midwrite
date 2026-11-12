// @expect MW1004 line=5 function=createCategories
// A private helper passed as a value may be called with anything: kept.
async function createCategories(db: Database, teamId: string) {
  const parents = await db.insert(categories).values([{ teamId }]).returning({ id: categories.id });
  await db.insert(categories).values(parents.map((p) => ({ teamId, parentId: p.id })));
}
export async function createTeam(db: Database, name: string) {
  register(createCategories);
  return db.transaction(async (tx) => {
    const [team] = await tx.insert(teams).values({ name }).returning({ id: teams.id });
    await createCategories(tx, team.id);
    return team.id;
  });
}

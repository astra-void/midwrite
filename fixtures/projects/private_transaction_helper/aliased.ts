// @expect MW1004 line=6 function=createCategories
// @expect MW9001 line=12 function=createTeam
// A private helper read into another binding: kept.
async function createCategories(db: Database, teamId: string) {
  const parents = await db.insert(categories).values([{ teamId }]).returning({ id: categories.id });
  await db.insert(categories).values(parents.map((p) => ({ teamId, parentId: p.id })));
}
const seed = createCategories;
export async function createTeam(db: Database, name: string) {
  return db.transaction(async (tx) => {
    const [team] = await tx.insert(teams).values({ name }).returning({ id: teams.id });
    await seed(tx, team.id);
    return team.id;
  });
}

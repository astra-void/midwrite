// A maintenance write can be harmless without being nonexistent. Keep this
// structural window; source review decides whether it is an intended risk.
// @expect MW1004 line=10 function=startJob
async function upgradeStorage(owner: string) {
  await db.update(credentials).set({ format: "v2" }).where({ owner });
}

export async function startJob(owner: string) {
  await upgradeStorage(owner);
  await db.insert(jobs).values({ owner });
}

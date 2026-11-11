import { and, eq } from "drizzle-orm";

import type { Database } from "../db.js";
import { members, projects } from "../schema/index.js";

export class ProjectStore {
  constructor(private readonly database: Database) {}

  async findBySlug(ownerId: string, slug: string) {
    const [found] = await this.database
      .select()
      .from(projects)
      .where(and(eq(projects.ownerId, ownerId), eq(projects.slug, slug)))
      .limit(1);
    return found;
  }

  async create(ownerId: string, slug: string) {
    const [created] = await this.database
      .insert(projects)
      .values({ ownerId, slug })
      .returning({ id: projects.id });
    return created;
  }

  async findMember(projectId: string, accountId: string) {
    const [found] = await this.database
      .select()
      .from(members)
      .where(and(eq(members.projectId, projectId), eq(members.accountId, accountId)))
      .limit(1);
    return found;
  }

  async addMember(projectId: string, accountId: string) {
    await this.database.insert(members).values({ projectId, accountId });
  }
}

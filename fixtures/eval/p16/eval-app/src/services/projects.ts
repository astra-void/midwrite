import { and, eq } from "drizzle-orm";
import { z } from "zod";

import { db } from "../db.js";
import { boards, projects } from "../schema/index.js";

export class SlugTakenError extends Error {
  constructor(readonly slug: string) {
    super(`slug "${slug}" is already taken in this workspace`);
  }
}

const Slug = z.string().regex(/^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/, "invalid slug");

// Unknown keys are rejected (a client cannot send `id` or `archived`).
const CreateProjectInput = z.strictObject({
  workspaceId: z.number().int().positive(),
  slug: Slug,
  name: z.string().trim().min(1).max(120),
});

// The same contract for boards, written with `.strict()`.
const CreateBoardInput = z
  .object({
    workspaceId: z.number().int().positive(),
    slug: Slug,
    name: z.string().trim().min(1).max(120),
  })
  .strict();

async function findProjectBySlug(workspaceId: number, slug: string) {
  const [row] = await db
    .select({ id: projects.id })
    .from(projects)
    .where(and(eq(projects.workspaceId, workspaceId), eq(projects.slug, slug)))
    .limit(1);
  return row ?? null;
}

async function findBoardBySlug(workspaceId: number, slug: string) {
  const [row] = await db
    .select({ id: boards.id })
    .from(boards)
    .where(and(eq(boards.workspaceId, workspaceId), eq(boards.slug, slug)))
    .limit(1);
  return row ?? null;
}

// P16-E01: creates a project unless the workspace already has one with the
// requested slug.
export async function createProject(raw: unknown) {
  const input = CreateProjectInput.parse(raw);
  if (await findProjectBySlug(input.workspaceId, input.slug)) {
    throw new SlugTakenError(input.slug);
  }
  const [project] = await db
    .insert(projects)
    .values({ workspaceId: input.workspaceId, slug: input.slug, name: input.name })
    .returning({ id: projects.id });
  if (!project) throw new Error("insert returned no row");
  return project.id;
}

// P16-E02: creates a board unless the workspace already has one with the
// requested slug.
export async function createBoard(raw: unknown) {
  const input = CreateBoardInput.parse(raw);
  if (await findBoardBySlug(input.workspaceId, input.slug)) {
    throw new SlugTakenError(input.slug);
  }
  const [board] = await db
    .insert(boards)
    .values({ workspaceId: input.workspaceId, slug: input.slug, name: input.name })
    .onConflictDoNothing({ target: [boards.workspaceId, boards.slug] })
    .returning({ id: boards.id });
  if (!board) throw new SlugTakenError(input.slug);
  return board.id;
}

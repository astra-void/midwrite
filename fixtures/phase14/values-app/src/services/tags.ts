import { eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { posts } from "../db/schema.js";

// Read the tag list, add the tag when it is missing, write the list back:
// two concurrent calls with different tags can lose one of them.
export async function addTag(postId: number, tag: string) {
  const [post] = await db.select().from(posts).where(eq(posts.id, postId)).limit(1);
  if (!post) throw new Error("no such post");
  const tags = post.tags.split(",");
  if (tags.includes(tag)) return false;
  await db
    .update(posts)
    .set({ tags: `${tags.join(",")},${tag}` })
    .where(eq(posts.id, postId));
  return true;
}

// The same read-modify-write under a row lock in a transaction.
export async function addTagLocked(postId: number, tag: string) {
  return db.transaction(async (tx) => {
    const [post] = await tx
      .select()
      .from(posts)
      .where(eq(posts.id, postId))
      .limit(1)
      .for("update");
    if (!post) throw new Error("no such post");
    const tags = post.tags.split(",");
    if (tags.includes(tag)) return false;
    await tx
      .update(posts)
      .set({ tags: post.tags + "," + tag })
      .where(eq(posts.id, postId));
    return true;
  });
}

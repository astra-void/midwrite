import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { bookmarks, comments, posts } from "../schema/index.js";

// E07: checks that the post exists, then adds a comment (comments.post_id
// references posts.id).
export async function addComment(postId: number, authorId: string, body: string) {
  const [post] = await db.select({ id: posts.id }).from(posts).where(eq(posts.id, postId)).limit(1);
  if (!post) throw new Error("no such post");
  const [comment] = await db
    .insert(comments)
    .values({ postId, authorId, body })
    .returning({ id: comments.id });
  return comment?.id;
}

// E08: checks that the post exists, then adds a bookmark (bookmarks.post_id
// is a plain column).
export async function addBookmark(postId: number, userId: string) {
  const [post] = await db.select({ id: posts.id }).from(posts).where(eq(posts.id, postId)).limit(1);
  if (!post) throw new Error("no such post");
  const [bookmark] = await db
    .insert(bookmarks)
    .values({ postId, userId })
    .returning({ id: bookmarks.id });
  return bookmark?.id;
}

// Removes a post with its bookmarks and comments.
export async function deletePost(postId: number) {
  await db.transaction(async (tx) => {
    await tx.delete(bookmarks).where(eq(bookmarks.postId, postId));
    await tx.delete(comments).where(eq(comments.postId, postId));
    await tx.delete(posts).where(eq(posts.id, postId));
  });
}

The chat deletion helpers delete related rows through several independently committed statements. A failure after message deletion but before the final chat deletion leaves a visible chat whose messages are already gone, even though the DELETE request failed.

Reviewed revision: `a8989b522fa3672a2ae52f053105e40993b36ca1`.

Source:
- [`deleteChatById` and `deleteAllChatsByUserId`](https://github.com/vercel/chatbot/blob/a8989b522fa3672a2ae52f053105e40993b36ca1/lib/db/queries.ts#L99-L140) delete votes, messages, streams and chats without a transaction.
- The [single-chat DELETE route](https://github.com/vercel/chatbot/blob/a8989b522fa3672a2ae52f053105e40993b36ca1/app/%28chat%29/api/chat/route.ts#L443-L470) calls the helper directly. The [history DELETE route](https://github.com/vercel/chatbot/blob/a8989b522fa3672a2ae52f053105e40993b36ca1/app/%28chat%29/api/history/route.ts#L36-L49) likewise calls the bulk helper.
- Related tables use [foreign keys without delete cascades](https://github.com/vercel/chatbot/blob/a8989b522fa3672a2ae52f053105e40993b36ca1/lib/db/schema.ts#L38-L82); the statements are separate cleanup steps, not one cascading delete.

Failure sequence to reproduce with a disposable test chat:
1. Create a chat with at least one message and vote.
2. Allow `db.delete(vote)` and `db.delete(message)` to complete, then inject a rejection in `db.delete(stream)` (or terminate the process before the chat delete).
3. Read the chat and messages again after removing the injected failure.
4. The chat still exists and is listed in history, but its messages and votes have already been deleted. The helper throws `bad_request:database` for a caught query failure.

Expected: deleting a chat and its dependent rows succeeds atomically or leaves all rows intact. A later successful retry can finish the deletion, but does not restore the contents after the failed request.

Wrapping each deletion helper in `db.transaction` and issuing every dependent statement through its `tx` handle would remove this partial-deletion window. The document-version and message-truncation cleanup helpers in the same file have similar multi-statement ordering and may benefit from the same approach.

Found with [Midwrite](https://github.com/astra-void/midwrite), an experimental static analyzer for partial-failure windows in TypeScript backends. I checked the reported operations and recovery path against the source. This is an AI-assisted source review, not a production observation or an executed fault-injection test; the steps above describe the proposed reproduction.

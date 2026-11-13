// Calls on what `getZeroDB()` returns run `DbRpcDO`'s methods, which forward
// to the `ZeroDB` object in `mainDo` (both declared in the test's
// `classContracts`). That object writes through `this.db`: the delete
// commits on its own, and `deleteUser` is one transaction, before the
// mailbox data is cleared.
// @expect MW1001 line=14 function=removeConnection
// @expect MW1001 line=20 function=removeUser
import { getZeroDB } from "../lib/server-utils";
import { clearMailboxChatData } from "../lib/chat";

export async function removeConnection(userId: string, connectionId: string) {
  const db = await getZeroDB(userId);
  await db.deleteConnection(connectionId);
  await clearMailboxChatData(connectionId);
}

export async function removeUser(userId: string) {
  const db = await getZeroDB(userId);
  await db.deleteUser();
  await clearMailboxChatData(userId);
}

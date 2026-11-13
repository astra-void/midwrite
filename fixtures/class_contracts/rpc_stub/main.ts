import { DurableObject, RpcTarget } from "cloudflare:workers";
import { connection, user, account } from "./schema";
import { createDb } from "./db";
import { and, eq } from "drizzle-orm";

export class DbRpcDO extends RpcTarget {
  constructor(
    private mainDo: ZeroDB,
    private userId: string,
  ) {
    super();
  }
  async deleteConnection(connectionId: string) {
    return await this.mainDo.deleteConnection(connectionId, this.userId);
  }
  async deleteUser() {
    return await this.mainDo.deleteUser(this.userId);
  }
}

class ZeroDB extends DurableObject {
  db = createDb("x").db;
  async setMetaData(userId: string) {
    return new DbRpcDO(this, userId);
  }
  async deleteConnection(connectionId: string, userId: string) {
    return await this.db.delete(connection).where(and(eq(connection.id, connectionId), eq(connection.userId, userId)));
  }
  async deleteUser(userId: string) {
    return await this.db.transaction(async (tx) => {
      await tx.delete(connection).where(eq(connection.userId, userId));
      await tx.delete(user).where(eq(user.id, userId));
    });
  }
}

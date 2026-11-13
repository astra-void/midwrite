import type { DbRpcDO } from "../main";
import { scopedRpc } from "./scoped-rpc";
export const getZeroDB = async (userId: string) => {
  return scopedRpc<DbRpcDO>(async () => ({}) as object);
};

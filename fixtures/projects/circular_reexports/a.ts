// `a` and `b` re-export each other with `export *`. Resolution terminates;
// `missing` exists in neither, so the call stays unknown.
// @expect none
export * from "./b";
export async function fromA() {}

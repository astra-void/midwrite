// Phase 16 Track A: the zod side of the differential matrix.
//
//   node run.mjs <zod package directory> <entry: index | v3 | v4> <matrix.json> <api: 3 | 4>
//
// Imports the pinned zod package from its directory (never from the
// network), builds every schema of the matrix that exists in `api`, and
// prints one JSON array: for each case and input, whether `safeParse`
// succeeded (or threw), its data (in the model's JSON value form), and
// whether `parse` threw. Inputs and outputs use the model's encodings: {"$undefined": true}
// is undefined, {"$map": [...]} a Map, {"$set": [...]} a Set.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const [dir, entry, matrixPath, api] = process.argv.slice(2);
const file = entry === "index" ? join(dir, "index.js") : join(dir, entry, "index.js");
const mod = await import(pathToFileURL(file).href);
const z = mod.z ?? mod.default;
const matrix = JSON.parse(readFileSync(matrixPath, "utf8"));

function decode(v) {
  if (Array.isArray(v)) return v.map(decode);
  if (v && typeof v === "object") {
    if (v.$undefined === true && Object.keys(v).length === 1) return undefined;
    if (Array.isArray(v.$map) && Object.keys(v).length === 1)
      return new Map(v.$map.map(([k, x]) => [decode(k), decode(x)]));
    if (Array.isArray(v.$set) && Object.keys(v).length === 1) return new Set(v.$set.map(decode));
    const out = {};
    for (const [k, x] of Object.entries(v)) out[k] = decode(x);
    return out;
  }
  return v;
}

function encode(v) {
  if (v === undefined) return { $undefined: true };
  if (Array.isArray(v)) return v.map(encode);
  if (v instanceof Map) return { $map: [...v].map(([k, x]) => [encode(k), encode(x)]) };
  if (v instanceof Set) return { $set: [...v].map(encode) };
  if (v && typeof v === "object") {
    const out = {};
    for (const [k, x] of Object.entries(v)) out[k] = encode(x);
    return out;
  }
  return v;
}

const constants = matrix.constants ?? {};
const names = Object.keys(constants);
const results = [];
for (const c of matrix.cases) {
  if (!c.apis.includes(Number(api))) continue;
  const schema = new Function("z", ...names, `return (${c.schema});`)(
    z,
    ...names.map((n) => constants[n]),
  );
  c.inputs.forEach((raw, i) => {
    const input = decode(raw);
    // A refinement callback that throws makes safeParse throw too.
    let r = { success: false };
    let safeThrew = false;
    try {
      r = schema.safeParse(input);
    } catch {
      safeThrew = true;
    }
    let threw = false;
    try {
      schema.parse(decode(raw));
    } catch {
      threw = true;
    }
    results.push({
      id: c.id,
      input: i,
      success: r.success,
      data: r.success ? encode(r.data) : null,
      parseThrew: threw,
      safeThrew,
    });
  });
}
process.stdout.write(JSON.stringify(results));

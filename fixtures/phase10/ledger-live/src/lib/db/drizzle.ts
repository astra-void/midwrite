import { drizzle } from "drizzle-orm/node-postgres";

import { configuredPool } from "./pool.js";
import * as schema from "./schema.js";

export const db = drizzle(configuredPool(), { schema });

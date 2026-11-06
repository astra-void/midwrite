import { createClient } from "@supabase/supabase-js";

export async function createAdminClient() {
  return createClient(process.env.SUPABASE_URL!, process.env.SUPABASE_KEY!);
}
// @expect none
